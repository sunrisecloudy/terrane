const std = @import("std");
const builtin = @import("builtin");
const core_api = @import("zig_core");
const sqlite = @cImport({
    @cInclude("sqlite3.h");
});

const max_request_bytes = 1024 * 1024;
const max_package_files = 32;
const max_migration_files = 16;
const runtime_version = "0.1.0";
const signature_prefix = "native-ai-webapp/sig/v1";
const network_credentials_denied_message = "network.request credentials are not allowed";
const control_auth_failure_limit = 3;
const control_auth_ban_ms: i64 = 60 * std.time.ms_per_s;
const control_auth_max_clients = 16;

pub fn main() !void {
    const allocator = std.heap.page_allocator;
    try enforceProductionStartupRules(allocator);
    const production_mode = isProductionMode(allocator);
    var control_token_config = if (production_mode)
        ControlTokenConfig{ .token = try allocator.dupe(u8, "") }
    else
        try initControlToken(allocator);
    defer control_token_config.deinit(allocator);
    const port = try parsePort(allocator);
    const address = try std.net.Address.parseIp("127.0.0.1", port);
    var server = try address.listen(.{ .reuse_address = true });
    defer server.deinit();
    var control_auth_tracker = ControlAuthTracker{};

    std.debug.print("native-ai zig server listening on http://127.0.0.1:{d}\n", .{port});
    if (control_token_config.token_file) |token_file| {
        std.debug.print("control token file: {s}\n", .{token_file});
    }

    while (true) {
        var connection = try server.accept();
        defer connection.stream.close();
        handleConnection(allocator, connection.stream, connection.address, &control_auth_tracker, control_token_config.token) catch |err| {
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

const ControlTokenConfig = struct {
    token: []u8,
    token_file: ?[]u8 = null,

    fn deinit(self: *ControlTokenConfig, allocator: std.mem.Allocator) void {
        allocator.free(self.token);
        if (self.token_file) |token_file| allocator.free(token_file);
    }
};

fn initControlToken(allocator: std.mem.Allocator) !ControlTokenConfig {
    const token = (try configuredControlTokenAlloc(allocator)) orelse try generateControlToken(allocator);
    errdefer allocator.free(token);
    const token_file = try controlTokenFilePath(allocator);
    errdefer allocator.free(token_file);
    try writeControlTokenFile(token_file, token);
    return .{ .token = token, .token_file = token_file };
}

fn configuredControlTokenAlloc(allocator: std.mem.Allocator) !?[]u8 {
    if (try envVarNonEmptyAlloc(allocator, "NATIVE_AI_SERVER_CONTROL_TOKEN")) |token| return token;
    return envVarNonEmptyAlloc(allocator, "PLATFORM_CONTROL_TOKEN");
}

fn controlTokenFilePath(allocator: std.mem.Allocator) ![]u8 {
    if (try argValueAlloc(allocator, "--token-file")) |path| return path;
    if (try envVarNonEmptyAlloc(allocator, "NATIVE_AI_SERVER_CONTROL_TOKEN_FILE")) |path| return path;
    if (try envVarNonEmptyAlloc(allocator, "PLATFORM_CONTROL_TOKEN_FILE")) |path| return path;

    if (builtin.os.tag == .windows) {
        const local_app_data = (try envVarNonEmptyAlloc(allocator, "LOCALAPPDATA")) orelse try homeRelativePath(allocator, &.{ "AppData", "Local" });
        defer allocator.free(local_app_data);
        return std.fs.path.join(allocator, &.{ local_app_data, "native-ai-webapp", "control.token" });
    }

    if (builtin.os.tag == .macos) {
        return homeRelativePath(allocator, &.{ "Library", "Application Support", "native-ai-webapp", "control.token" });
    }

    const runtime_dir = (try envVarNonEmptyAlloc(allocator, "XDG_RUNTIME_DIR")) orelse return error.ControlTokenPathRequired;
    defer allocator.free(runtime_dir);
    return std.fs.path.join(allocator, &.{ runtime_dir, "native-ai-webapp", "control.token" });
}

fn homeRelativePath(allocator: std.mem.Allocator, parts: []const []const u8) ![]u8 {
    const home = (try envVarNonEmptyAlloc(allocator, "HOME")) orelse return error.ControlTokenPathRequired;
    defer allocator.free(home);

    var segments = try allocator.alloc([]const u8, parts.len + 1);
    defer allocator.free(segments);
    segments[0] = home;
    for (parts, 0..) |part, index| {
        segments[index + 1] = part;
    }
    return std.fs.path.join(allocator, segments);
}

fn argValueAlloc(allocator: std.mem.Allocator, flag: []const u8) !?[]u8 {
    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);

    var index: usize = 1;
    while (index < args.len) : (index += 1) {
        if (std.mem.eql(u8, args[index], flag)) {
            if (index + 1 >= args.len) return error.MissingControlTokenFileValue;
            return try allocator.dupe(u8, args[index + 1]);
        }
        if (std.mem.startsWith(u8, args[index], flag) and args[index].len > flag.len and args[index][flag.len] == '=') {
            return try allocator.dupe(u8, args[index][flag.len + 1 ..]);
        }
    }
    return null;
}

fn envVarNonEmptyAlloc(allocator: std.mem.Allocator, name: []const u8) !?[]u8 {
    const value = std.process.getEnvVarOwned(allocator, name) catch |err| switch (err) {
        error.EnvironmentVariableNotFound => return null,
        else => return err,
    };
    if (value.len == 0) {
        allocator.free(value);
        return null;
    }
    return value;
}

fn generateControlToken(allocator: std.mem.Allocator) ![]u8 {
    var random_bytes: [32]u8 = undefined;
    std.crypto.random.bytes(&random_bytes);
    return base64UrlNoPadAlloc(allocator, &random_bytes);
}

fn base64UrlNoPadAlloc(allocator: std.mem.Allocator, input: []const u8) ![]u8 {
    const output = try allocator.alloc(u8, std.base64.url_safe_no_pad.Encoder.calcSize(input.len));
    _ = std.base64.url_safe_no_pad.Encoder.encode(output, input);
    return output;
}

fn writeControlTokenFile(token_file: []const u8, token: []const u8) !void {
    if (std.fs.path.dirname(token_file)) |parent| {
        try std.fs.cwd().makePath(parent);
    }
    var file = try std.fs.cwd().createFile(token_file, .{ .truncate = true, .mode = 0o600 });
    defer file.close();
    if (builtin.os.tag != .windows) {
        try file.chmod(0o600);
    }
    try file.writeAll(token);
    try file.writeAll("\n");
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
        "--token-file",
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

fn handleConnection(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
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
        return handleWebappInstall(allocator, stream, parsed.body, parsed.control_token, client_address, control_auth_tracker, expected_control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and
        (std.mem.startsWith(u8, parsed.path, "/packages/") or std.mem.startsWith(u8, parsed.path, "/control/packages/")))
    {
        return handlePackageControlEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token, client_address, control_auth_tracker, expected_control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and appIdFromRollbackPath(parsed.path) != null) {
        return handleAppRollbackEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token, client_address, control_auth_tracker, expected_control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/control/command")) {
        return handleControlCommand(allocator, stream, parsed.body, parsed.control_token, client_address, control_auth_tracker, expected_control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and
        (std.mem.startsWith(u8, parsed.path, "/db/") or std.mem.startsWith(u8, parsed.path, "/control/db/")))
    {
        return handleDbControlEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token, client_address, control_auth_tracker, expected_control_token);
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
    if (params.object.get("appId") != null) {
        return writeBridgeErrorWithDetails(allocator, stream, id, "invalid_request", "Bridge params must not include appId; app id is channel-derived", "{\"field\":\"appId\"}");
    }
    const params_json_for_audit = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json_for_audit);

    if (!isAllowedRuntimeBridgeMethod(method)) {
        if (isKnownUnsupportedBridgeMethod(method)) {
            const details_json = try methodDetailsJsonAlloc(allocator, method);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "platform_unsupported", "Bridge method is not implemented on zig-server", details_json);
        }
        const details_json = try methodDetailsJsonAlloc(allocator, method);
        defer allocator.free(details_json);
        return writeBridgeErrorWithDetails(allocator, stream, id, "unknown_method", "Unknown bridge method", details_json);
    }

    const compatible_runtime = bridgeRuntimeCompatible(allocator, channel_app_id) catch {
        const error_json = try bridgeErrorJsonAlloc(allocator, "runtime_compatibility_unavailable", "Runtime compatibility could not be evaluated");
        defer allocator.free(error_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "runtime_compatibility_unavailable", "Runtime compatibility could not be evaluated");
    };
    if (!compatible_runtime) {
        const error_json = try bridgeErrorJsonAlloc(allocator, "runtime_version_incompatible", "App runtimeVersion is not compatible with the zig-server runtime");
        defer allocator.free(error_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "runtime_version_incompatible", "App runtimeVersion is not compatible with the zig-server runtime");
    }

    if (try takeInjectedFaultAlloc(allocator, channel_app_id, session_id, method)) |fault| {
        defer freeFaultInjection(allocator, fault);
        const error_json = try faultBridgeErrorJsonAlloc(allocator, fault, channel_app_id, method);
        defer allocator.free(error_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, fault.code, fault.message);
    }

    if (permissionForBridgeMethod(method)) |permission| {
        const permitted = bridgePermissionApproved(allocator, channel_app_id, permission) catch false;
        if (!permitted) {
            const details_json = try permissionDetailsJsonAlloc(allocator, channel_app_id, method, permission);
            defer allocator.free(details_json);
            const error_json = try bridgeErrorJsonWithDetailsAlloc(allocator, "permission_denied", "Bridge method requires an approved app permission", details_json);
            defer allocator.free(error_json);
            logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
                std.debug.print("bridge audit write failed: {}\n", .{err});
            };
            return writeBridgeErrorWithDetails(allocator, stream, id, "permission_denied", "Bridge method requires an approved app permission", details_json);
        }
    }

    const budget_violation = enforceBridgeResourceBudget(allocator, channel_app_id, method, params) catch {
        const error_json = try bridgeErrorJsonAlloc(allocator, "resource_budget_unavailable", "Resource budget could not be evaluated");
        defer allocator.free(error_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "resource_budget_unavailable", "Resource budget could not be evaluated");
    };
    if (budget_violation) |violation| {
        const details_json = try resourceBudgetDetailsJsonAlloc(allocator, violation);
        defer allocator.free(details_json);
        const error_json = try bridgeErrorJsonWithDetailsAlloc(allocator, "resource_budget_exceeded", violation.message, details_json);
        defer allocator.free(error_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json_for_audit, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeErrorWithDetails(allocator, stream, id, "resource_budget_exceeded", violation.message, details_json);
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
        const result_json = try serverCapabilitiesForAppJson(allocator, channel_app_id);
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

    if (std.mem.eql(u8, method, "dialog.openFile") or std.mem.eql(u8, method, "dialog.saveFile")) {
        return handleDialogBridge(allocator, stream, id, method, params, channel_app_id, session_id);
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

const ResourceBudgetViolation = struct {
    message: []const u8,
    app_id: []const u8,
    budget: []const u8,
    current: i64,
    max: i64,
};

fn enforceBridgeResourceBudget(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    method: []const u8,
    params: std.json.Value,
) !?ResourceBudgetViolation {
    const manifest_json = activeManifestJsonAlloc(allocator, app_id) catch |err| switch (err) {
        error.AppNotInstalled => return null,
        else => return err,
    };
    defer allocator.free(manifest_json);
    const active_install_id = activeInstallIdForAppAlloc(allocator, app_id) catch null;
    defer if (active_install_id) |install_id| allocator.free(install_id);

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{}) catch {
        return error.InvalidResourceBudget;
    };
    defer parsed.deinit();

    if (resourceBudgetLimit(parsed.value, "maxBridgeCallsPerMinute")) |limit| {
        const count = try countBridgeCallsSince(allocator, app_id, active_install_id, null);
        if (count >= limit) {
            return .{
                .message = "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute",
                .app_id = app_id,
                .budget = "maxBridgeCallsPerMinute",
                .current = count + 1,
                .max = limit,
            };
        }
    }

    if (std.mem.eql(u8, method, "network.request")) {
        if (resourceBudgetLimit(parsed.value, "maxNetworkRequestsPerMinute")) |limit| {
            const count = try countBridgeCallsSince(allocator, app_id, active_install_id, "network.request");
            if (count >= limit) {
                return .{
                    .message = "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute",
                    .app_id = app_id,
                    .budget = "maxNetworkRequestsPerMinute",
                    .current = count + 1,
                    .max = limit,
                };
            }
        }
    }

    if (std.mem.eql(u8, method, "app.log")) {
        if (resourceBudgetLimit(parsed.value, "maxLogLinesPerMinute")) |limit| {
            const count = try countBridgeCallsSince(allocator, app_id, active_install_id, "app.log");
            if (count >= limit) {
                return .{
                    .message = "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute",
                    .app_id = app_id,
                    .budget = "maxLogLinesPerMinute",
                    .current = count + 1,
                    .max = limit,
                };
            }
        }
    }

    if (std.mem.eql(u8, method, "storage.set")) {
        if (resourceBudgetLimit(parsed.value, "maxStorageBytes")) |limit| {
            const key = valueString(params.object.get("key")) orelse return null;
            const value_json = if (params.object.get("value")) |value|
                try jsonValueAlloc(allocator, value)
            else
                try allocator.dupe(u8, "null");
            defer allocator.free(value_json);
            const projected_bytes = try storageBytesAfterSet(allocator, app_id, key, value_json);
            if (projected_bytes > limit) {
                return .{
                    .message = "Storage write exceeds manifest.resourceBudget.maxStorageBytes",
                    .app_id = app_id,
                    .budget = "maxStorageBytes",
                    .current = projected_bytes,
                    .max = limit,
                };
            }
        }
    }

    return null;
}

fn resourceBudgetLimit(manifest: std.json.Value, field: []const u8) ?i64 {
    if (manifest != .object) return null;
    const resource_budget = manifest.object.get("resourceBudget") orelse return null;
    if (resource_budget != .object) return null;
    const limit = valueI64(resource_budget.object.get(field)) orelse return null;
    if (limit < 0) return null;
    return limit;
}

fn countBridgeCallsSince(allocator: std.mem.Allocator, app_id: []const u8, install_id: ?[]const u8, method: ?[]const u8) !i64 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    const sql: [*:0]const u8 = if (method != null and install_id != null)
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND install_id = ? AND method = ? AND julianday(created_at) >= julianday('now','-60 seconds')"
    else if (method != null)
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND julianday(created_at) >= julianday('now','-60 seconds')"
    else if (install_id != null)
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND install_id = ? AND julianday(created_at) >= julianday('now','-60 seconds')"
    else
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND julianday(created_at) >= julianday('now','-60 seconds')";
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (install_id) |actual_install_id| {
        bindText(statement, 2, actual_install_id);
        if (method) |actual_method| bindText(statement, 3, actual_method);
    } else if (method) |actual_method| {
        bindText(statement, 2, actual_method);
    }
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn countBridgeErrorsSince(allocator: std.mem.Allocator, app_id: []const u8, install_id: []const u8, code: []const u8) !i64 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const pattern = try std.fmt.allocPrint(allocator, "%\"code\":\"{s}\"%", .{code});
    defer allocator.free(pattern);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND install_id = ? AND error_json LIKE ? AND julianday(created_at) >= julianday('now','-60 seconds')",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, install_id);
    bindText(statement, 3, pattern);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn storageBytesAfterSet(allocator: std.mem.Allocator, app_id: []const u8, key: []const u8, value_json: []const u8) !i64 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0), " ++
            "COALESCE((SELECT LENGTH(CAST(value_json AS BLOB)) FROM app_storage WHERE app_id = ? AND key = ?), 0) " ++
            "FROM app_storage WHERE app_id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    bindText(statement, 3, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    const current = sqlite.sqlite3_column_int64(statement, 0);
    const existing = sqlite.sqlite3_column_int64(statement, 1);
    const retained = if (current > existing) current - existing else 0;
    return retained + @as(i64, @intCast(value_json.len));
}

fn bridgeRuntimeCompatible(allocator: std.mem.Allocator, app_id: []const u8) !bool {
    const manifest_json = activeManifestJsonAlloc(allocator, app_id) catch |err| switch (err) {
        error.AppNotInstalled => return true,
        else => return err,
    };
    defer allocator.free(manifest_json);

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{}) catch {
        return error.InvalidRuntimeCompatibility;
    };
    defer parsed.deinit();
    const app_runtime_version = valueString(parsed.value.object.get("runtimeVersion")) orelse return false;
    return runtimeVersionsCompatible(runtime_version, app_runtime_version) or allowRuntimeMismatch(allocator);
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
            const details_json = try storagePrefixDetailsJsonAlloc(allocator, key, prefix);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix", details_json);
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
            const details_json = try storagePrefixDetailsJsonAlloc(allocator, key, prefix);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix", details_json);
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
            const details_json = try storagePrefixDetailsJsonAlloc(allocator, key, prefix);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix", details_json);
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
            const details_json = try storagePrefixDetailsJsonAlloc(allocator, prefix_param, prefix);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix", details_json);
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
            const details_json = try toastLevelDetailsJsonAlloc(allocator, level);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "invalid_request", "notification.toast level must be info, success, warning, or error", details_json);
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

    const deny_details = networkRequestDenyDetailsJsonAlloc(allocator, app_id, url, method, params) catch |err| switch (err) {
        error.InvalidNetworkUrl => {
            const details_json = try networkUrlDetailsJsonAlloc(allocator, url);
            defer allocator.free(details_json);
            return writeBridgeErrorWithDetails(allocator, stream, id, "invalid_request", "network.request url must be absolute", details_json);
        },
        else => return writeBridgeError(allocator, stream, id, "network_policy_denied", "network.request is outside manifest.networkPolicy"),
    };
    if (deny_details) |details_json| {
        defer allocator.free(details_json);
        const error_json = try bridgeErrorJsonWithDetailsAlloc(allocator, "network_policy_denied", "network.request is outside manifest.networkPolicy", details_json);
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, "network.request", params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeErrorWithDetails(allocator, stream, id, "network_policy_denied", "network.request is outside manifest.networkPolicy", details_json);
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
    if (try networkResponsePolicyErrorAlloc(allocator, app_id, url, method, params, result_json)) |policy_error| {
        defer freeNetworkPolicyBridgeError(allocator, policy_error);
        const error_json = try bridgeErrorJsonWithDetailsAlloc(allocator, policy_error.code, policy_error.message, policy_error.details_json);
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, "network.request", params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeErrorWithDetails(allocator, stream, id, policy_error.code, policy_error.message, policy_error.details_json);
    }
    const response_payload_json = try networkResponsePayloadJsonAlloc(allocator, result_json);
    defer allocator.free(response_payload_json);
    logBridgeCall(allocator, app_id, session_id, "network.request", params_json, response_payload_json, null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return writeBridgeOkRaw(allocator, stream, id, response_payload_json);
}

fn handleDialogBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    app_id: []const u8,
    session_id: ?[]const u8,
) !void {
    const dialog_type = dialogTypeForBridgeMethod(method) orelse {
        return writeBridgeError(allocator, stream, id, "unknown_method", "Unknown dialog method");
    };
    const params_json = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json);

    const result_json = (try dialogMockResultJsonAlloc(allocator, app_id, session_id, dialog_type)) orelse {
        if (std.mem.eql(u8, dialog_type, "saveFile")) {
            logBridgeCall(allocator, app_id, session_id, method, params_json, "{\"ok\":true}", null) catch |err| {
                std.debug.print("bridge audit write failed: {}\n", .{err});
            };
            return writeBridgeOkRaw(allocator, stream, id, "{\"ok\":true}");
        }
        const error_json = try bridgeErrorJsonAlloc(allocator, "dialog.mock_missing", "No dialog.openFile mock is registered");
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "dialog.mock_missing", "No dialog.openFile mock is registered");
    };
    defer allocator.free(result_json);
    logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
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
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
    authorizeControlRequest(control_auth_tracker, client_address, expected_control_token, provided_token) catch |err| {
        return rejectControlAuth(allocator, stream, control_auth_tracker, client_address, "/webapps/install", "platform.install_webapp_package", err);
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
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
    const tool = controlToolForPackagePath(path);
    authorizeControlRequest(control_auth_tracker, client_address, expected_control_token, provided_token) catch |err| {
        return rejectControlAuth(allocator, stream, control_auth_tracker, client_address, path, tool, err);
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
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
    const tool = "platform.rollback_webapp";
    authorizeControlRequest(control_auth_tracker, client_address, expected_control_token, provided_token) catch |err| {
        return rejectControlAuth(allocator, stream, control_auth_tracker, client_address, path, tool, err);
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
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
    const audit_tool = controlToolForDbPath(path);
    authorizeControlRequest(control_auth_tracker, client_address, expected_control_token, provided_token) catch |err| {
        return rejectControlAuth(allocator, stream, control_auth_tracker, client_address, path, audit_tool, err);
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
    client_address: std.net.Address,
    control_auth_tracker: *ControlAuthTracker,
    expected_control_token: []const u8,
) !void {
    authorizeControlRequest(control_auth_tracker, client_address, expected_control_token, provided_token) catch |err| {
        return rejectControlAuth(allocator, stream, control_auth_tracker, client_address, "/control/command", "control.command", err);
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
    if (std.mem.eql(u8, tool, "platform.list_targets")) {
        const result_json = "{\"targets\":[{\"id\":\"server\",\"platform\":\"server\",\"status\":\"available\",\"runtimeVersion\":\"0.1.0\"}]}";
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.launch")) {
        const result_json = "{\"ok\":true,\"target\":\"server\",\"status\":\"running\"}";
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.stop")) {
        const result_json = "{\"ok\":true,\"target\":\"server\",\"status\":\"running\",\"note\":\"server stop is managed by the owning process\"}";
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.reload_runtime")) {
        const result_json = "{\"ok\":true,\"target\":\"server\",\"status\":\"reloaded\"}";
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
    if (std.mem.eql(u8, tool, "platform.open_webapp")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.open_webapp requires appId");
        };
        const result_json = openWebappControl(allocator, app_id) catch |err| switch (err) {
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.CapabilityUnavailable => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "capability_unavailable", args_json, null);
                return writeControlError(allocator, stream, 400, "capability_unavailable", "Required runtime capability is unavailable on server");
            },
            else => return err,
        };
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
    if (std.mem.eql(u8, tool, "platform.uninstall_webapp")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.uninstall_webapp requires appId");
        };
        const confirm = controlBoolArg(args, "confirm") orelse false;
        const result_json = uninstallWebappControl(allocator, app_id, confirm, controlStringArg(args, "actor") orelse "codex") catch |err| switch (err) {
            error.ConfirmationRequired => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "confirmation_required", args_json, null);
                return writeControlError(allocator, stream, 400, "confirmation_required", "platform.uninstall_webapp requires confirm: true");
            },
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
    if (std.mem.eql(u8, tool, "platform.approve_webapp_update")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.approve_webapp_update requires appId");
        };
        const install_id = controlStringArg(args, "installId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.approve_webapp_update requires installId");
        };
        const result_json = approveWebappUpdateControl(allocator, app_id, install_id) catch |err| switch (err) {
            error.InstallNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "install_not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "install_not_found", "Install was not found for app");
            },
            error.InstallStatusInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "install_status_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "install_status_invalid", "Install cannot be approved from its current status");
            },
            error.ApprovalNotRequired => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "approval_not_required", args_json, null);
                return writeControlError(allocator, stream, 400, "approval_not_required", "Install does not require approval");
            },
            error.InvalidMigration => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_migration", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_migration", "Pending update migration is invalid or incomplete");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.quarantine_webapp")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.quarantine_webapp requires appId");
        };
        const result_json = quarantineWebappControl(allocator, app_id, controlStringArg(args, "installId"), controlStringArg(args, "reason") orelse "manual quarantine") catch |err| switch (err) {
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.InstallNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "install_not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "install_not_found", "Install was not found for app");
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
    if (std.mem.eql(u8, tool, "runtime.dialog_mock_set")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.dialog_mock_set requires args");
        };
        const result_json = insertDialogMockControl(allocator, args_value) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.dialog_mock_set requires dialogType or method");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.reset_webapp") or std.mem.eql(u8, tool, "runtime.storage_reset")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "reset requires appId");
        };
        const confirm = controlBoolArg(args, "confirm") orelse false;
        if (!confirm) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "confirmation_required", args_json, null);
            return writeControlError(allocator, stream, 400, "confirmation_required", "reset requires confirm: true");
        }
        const result_json = if (std.mem.eql(u8, tool, "platform.reset_webapp"))
            try resetWebappControl(allocator, app_id)
        else
            try resetAppStorageControl(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.storage_get")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.storage_get requires appId");
        };
        const key = controlStringArg(args, "key") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.storage_get requires key");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, key))) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "permission_denied", args_json, null);
            return writeControlError(allocator, stream, 400, "permission_denied", "Storage key must begin with app storage prefix");
        }
        const default_value = if (args) |args_value| args_value.object.get("defaultValue") else null;
        const result_json = try storageGetResultJson(allocator, app_id, key, default_value);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.storage_set")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.storage_set requires appId");
        };
        const key = controlStringArg(args, "key") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.storage_set requires key");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, key))) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "permission_denied", args_json, null);
            return writeControlError(allocator, stream, 400, "permission_denied", "Storage key must begin with app storage prefix");
        }
        const value_json = if (args) |args_value| blk: {
            const value = args_value.object.get("value") orelse break :blk try allocator.dupe(u8, "null");
            break :blk try jsonValueAlloc(allocator, value);
        } else try allocator.dupe(u8, "null");
        defer allocator.free(value_json);
        try storageSet(app_id, key, value_json);
        const result_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"bytesWritten\":{d}}}", .{value_json.len});
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.snapshot")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.snapshot requires appId");
        };
        const result_json = runtimeSnapshotControl(allocator, app_id) catch |err| switch (err) {
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
    if (std.mem.eql(u8, tool, "runtime.query")) {
        const result_json = runtimeQueryControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.query requires appId");
            },
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
    if (std.mem.eql(u8, tool, "runtime.screenshot")) {
        const result_json = runtimeScreenshotControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.screenshot requires appId");
            },
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
    if (std.mem.eql(u8, tool, "runtime.click") or std.mem.eql(u8, tool, "runtime.type") or std.mem.eql(u8, tool, "runtime.set_value") or std.mem.eql(u8, tool, "runtime.drag")) {
        const result_json = runtimeTargetControl(allocator, tool, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "Runtime target command requires appId");
            },
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.SelectorNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "selector.not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "selector.not_found", "Runtime target was not found in installed package HTML");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.press_key")) {
        const result_json = try runtimePressKeyControl(allocator, args);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.wait_for")) {
        const result_json = try runtimeWaitForControl(allocator, args);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_visible")) {
        const result_json = assertRuntimeVisibleControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_visible requires appId");
            },
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.SelectorNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "selector.not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "selector.not_found", "Expected runtime target is not visible");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_text")) {
        const result_json = assertRuntimeTextControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_text requires appId and text");
            },
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.TextNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "text.not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "text.not_found", "Expected text was not found in installed package HTML");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.accessibility_snapshot")) {
        const result_json = runtimeAccessibilitySnapshotControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.accessibility_snapshot requires appId");
            },
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
    if (std.mem.eql(u8, tool, "runtime.run_accessibility_audit")) {
        const result_json = runtimeAccessibilityAuditControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.run_accessibility_audit requires appId");
            },
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
    if (std.mem.eql(u8, tool, "runtime.assert_accessibility")) {
        const result_json = runtimeAssertAccessibilityControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_accessibility requires appId");
            },
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.AccessibilityFailed => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "accessibility_failed", args_json, null);
                return writeControlError(allocator, stream, 400, "accessibility_failed", "Accessibility assertion failed");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.run_smoke_tests")) {
        const result_json = runtimeRunSmokeTestsControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.run_smoke_tests requires appId");
            },
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
    if (std.mem.eql(u8, tool, "runtime.run_microtest")) {
        const result_json = runtimeRunMicrotestControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.run_microtest requires spec or microtestPath");
            },
            error.InvalidMicrotest => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_microtest", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_microtest", "Micro-test must target at least one app");
            },
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
    if (std.mem.eql(u8, tool, "platform.run_platform_smoke")) {
        const result_json = platformRunSmokeControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "platform.run_platform_smoke requires spec or smokePath");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.run_repair_loop")) {
        const result_json = platformRunRepairLoopControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "platform.run_repair_loop requires an inline package");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.resource_usage")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.resource_usage requires appId");
        };
        const result_json = try runtimeResourceUsageControl(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.console_logs")) {
        const result_json = try consoleLogsControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.event_log")) {
        const result_json = try runtimeEventLogControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.clear_logs")) {
        const result_json = try clearRuntimeLogsControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_no_console_errors")) {
        const result_json = try assertNoConsoleErrorsControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.notification_capture")) {
        const result_json = try notificationCaptureControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.timer_advance")) {
        const result_json = try timerAdvanceControl(allocator, args);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.fault_inject")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.fault_inject requires args");
        };
        const result_json = insertFaultInjectionControl(allocator, args_value) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.fault_inject requires a bridge method");
            },
            error.UnknownBridgeMethod => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "unknown_method", args_json, null);
                return writeControlError(allocator, stream, 400, "unknown_method", "Unknown bridge method");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.compare_snapshot")) {
        const result_json = compareSnapshotControl(allocator, args) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.compare_snapshot requires left/right snapshots or snapshot ids");
            },
            error.SnapshotNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_not_found", "Snapshot was not found");
            },
            error.SnapshotInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_invalid", "Snapshot cannot be parsed");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.call_bridge")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.call_bridge requires args");
        };
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.call_bridge requires appId");
        };
        const method = controlStringArg(args, "method") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.call_bridge requires method");
        };
        const id = controlStringArg(args, "id") orelse "control_call_bridge";
        const params_value = if (args_value.object.get("params")) |params| blk: {
            if (params != .object) {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.call_bridge params must be an object");
            }
            break :blk params;
        } else null;
        const result_json = try callBridgeControl(allocator, app_id, controlStringArg(args, "sessionId"), id, method, params_value);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.core_step")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.core_step requires args");
        };
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.core_step requires appId");
        };
        const event_value = args_value.object.get("event") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.core_step requires event");
        };
        const result_json = try coreStepControl(allocator, app_id, controlStringArg(args, "sessionId"), controlStringArg(args, "id") orelse "control_core_step", event_value);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.core_snapshot")) {
        const result_json = try coreSnapshotControl(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.replay_events")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.replay_events requires args");
        };
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.replay_events requires appId");
        };
        const events = args_value.object.get("events") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.replay_events requires events");
        };
        if (events != .array) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.replay_events events must be an array");
        }
        const result_json = try replayEventsControl(allocator, app_id, events);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_storage")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_storage requires args");
        };
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_storage requires appId");
        };
        const key = controlStringArg(args, "key") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_storage requires key");
        };
        const expected = args_value.object.get("value") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_storage requires value");
        };
        const result_json = assertStorageControl(allocator, app_id, key, expected) catch |err| switch (err) {
            error.AssertionFailed => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "assertion_failed", args_json, null);
                return writeControlError(allocator, stream, 400, "assertion_failed", "Expected storage value was not found");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_bridge_call")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_bridge_call requires appId");
        };
        const method = controlStringArg(args, "method") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_bridge_call requires method");
        };
        const result_json = assertBridgeCallControl(allocator, app_id, method) catch |err| switch (err) {
            error.AssertionFailed => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "assertion_failed", args_json, null);
                return writeControlError(allocator, stream, 400, "assertion_failed", "Expected bridge call was not found");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.assert_core_action")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_core_action requires args");
        };
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_core_action requires appId");
        };
        if (args_value.object.get("match")) |match| {
            if (match != .object and match != .null) {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.assert_core_action match must be an object");
            }
        }
        const result_json = assertCoreActionControl(allocator, app_id, controlStringArg(args, "type"), args_value.object.get("match")) catch |err| switch (err) {
            error.AssertionFailed => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "assertion_failed", args_json, null);
                return writeControlError(allocator, stream, 400, "assertion_failed", "Expected core action was not found");
            },
            else => return err,
        };
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
    if (std.mem.eql(u8, tool, "db.export_backup")) {
        const result_json = try dbBackupExportJson(allocator);
        defer allocator.free(result_json);
        recordBackupExport(allocator, result_json) catch |err| {
            std.debug.print("backup export record failed: {}\n", .{err});
        };
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.import_backup")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.import_backup requires args");
        };
        const backup = args_value.object.get("backup") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.import_backup requires backup");
        };
        const result_json = importBackupControl(allocator, backup) catch |err| switch (err) {
            error.InvalidBackup => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_backup", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_backup", "Backup document is invalid or incomplete");
            },
            else => return err,
        };
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

const ControlClientKey = struct {
    family: std.posix.sa_family_t = 0,
    bytes: [16]u8 = [_]u8{0} ** 16,
    len: u8 = 0,

    fn eql(self: ControlClientKey, other: ControlClientKey) bool {
        return self.family == other.family and
            self.len == other.len and
            std.mem.eql(u8, self.bytes[0..self.len], other.bytes[0..other.len]);
    }
};

const ControlAuthEntry = struct {
    key: ControlClientKey = .{},
    failure_count: u8 = 0,
    banned_until_ms: i64 = 0,
};

const ControlAuthTracker = struct {
    entries: [control_auth_max_clients]ControlAuthEntry = [_]ControlAuthEntry{.{}} ** control_auth_max_clients,

    fn entryFor(self: *ControlAuthTracker, key: ControlClientKey) *ControlAuthEntry {
        for (&self.entries) |*entry| {
            if (entry.key.eql(key)) return entry;
        }
        for (&self.entries) |*entry| {
            if (entry.key.len == 0) {
                entry.key = key;
                return entry;
            }
        }
        self.entries[0] = .{ .key = key };
        return &self.entries[0];
    }

    fn find(self: *ControlAuthTracker, key: ControlClientKey) ?*ControlAuthEntry {
        for (&self.entries) |*entry| {
            if (entry.key.eql(key)) return entry;
        }
        return null;
    }

    fn retryAfterSeconds(self: *ControlAuthTracker, key: ControlClientKey, now_ms: i64) i64 {
        const entry = self.find(key) orelse return 60;
        if (entry.banned_until_ms <= now_ms) return 0;
        const remaining_ms = entry.banned_until_ms - now_ms;
        return @divTrunc(remaining_ms + std.time.ms_per_s - 1, std.time.ms_per_s);
    }
};

fn authorizeControlRequest(
    tracker: *ControlAuthTracker,
    client_address: std.net.Address,
    expected_token: []const u8,
    provided_token: ?[]const u8,
) !void {
    const key = controlClientKey(client_address);
    return authorizeControlTokenValue(tracker, key, expected_token, provided_token, std.time.milliTimestamp());
}

fn authorizeControlTokenValue(
    tracker: *ControlAuthTracker,
    key: ControlClientKey,
    expected: []const u8,
    provided_token: ?[]const u8,
    now_ms: i64,
) error{ ControlAuthRequired, ControlConnectionBanned }!void {
    const entry = tracker.entryFor(key);
    if (entry.banned_until_ms > now_ms) return error.ControlConnectionBanned;

    requireControlTokenValue(expected, provided_token) catch {
        if (entry.failure_count < std.math.maxInt(u8)) entry.failure_count += 1;
        if (entry.failure_count >= control_auth_failure_limit) {
            entry.banned_until_ms = now_ms + control_auth_ban_ms;
            return error.ControlConnectionBanned;
        }
        return error.ControlAuthRequired;
    };

    entry.failure_count = 0;
    entry.banned_until_ms = 0;
}

fn controlClientKey(address: std.net.Address) ControlClientKey {
    var key = ControlClientKey{ .family = address.any.family };
    switch (address.any.family) {
        std.posix.AF.INET => {
            const bytes: *const [4]u8 = @ptrCast(&address.in.sa.addr);
            @memcpy(key.bytes[0..4], bytes[0..4]);
            key.len = 4;
        },
        std.posix.AF.INET6 => {
            @memcpy(key.bytes[0..16], address.in6.sa.addr[0..16]);
            key.len = 16;
        },
        else => {
            key.len = 1;
            key.bytes[0] = 0;
        },
    }
    return key;
}

fn rejectControlAuth(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    tracker: *ControlAuthTracker,
    client_address: std.net.Address,
    path: []const u8,
    tool: []const u8,
    auth_error: anyerror,
) !void {
    switch (auth_error) {
        error.ControlConnectionBanned => {
            const code = "control_connection_banned";
            auditControlCommand(allocator, path, tool, "rejected", code, null, null);
            const retry_after_seconds = tracker.retryAfterSeconds(controlClientKey(client_address), std.time.milliTimestamp());
            return writeControlBanError(allocator, stream, retry_after_seconds);
        },
        else => {
            auditControlCommand(allocator, path, tool, "rejected", "control_auth_required", null, null);
            return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
        },
    }
}

fn requireControlToken(allocator: std.mem.Allocator, provided_token: ?[]const u8) !void {
    const expected = try std.process.getEnvVarOwned(allocator, "NATIVE_AI_SERVER_CONTROL_TOKEN");
    defer allocator.free(expected);
    return requireControlTokenValue(expected, provided_token);
}

fn requireControlTokenValue(expected: []const u8, provided_token: ?[]const u8) error{ControlAuthRequired}!void {
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

fn controlI64Arg(args: ?std.json.Value, name: []const u8) ?i64 {
    const value = args orelse return null;
    if (value != .object) return null;
    return valueI64(value.object.get(name));
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
    try validateServerPackageFileList(allocator, files, errors);

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
    if (manifestTrustLevelIs(manifest, "bundled") and manifest.object.get("contentRating") == null) {
        try errors.append(allocator, "missing_content_rating");
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
                const permission_name = valueString(permission) orelse {
                    try errors.append(allocator, "invalid_permissions");
                    continue;
                };
                if (!isKnownPackagePermission(permission_name)) try errors.append(allocator, "unknown_permission");
            }
        }
    }
    if (manifest.object.get("capabilities")) |capabilities| {
        try validateServerCapabilities(allocator, manifest, capabilities, errors);
    }
    if (manifest.object.get("contentRating")) |content_rating| {
        try validateServerContentRating(allocator, content_rating, errors);
    }
    if (manifest.object.get("resourceBudget")) |resource_budget| {
        try validateServerResourceBudget(allocator, resource_budget, errors);
        try validateServerPackageBudget(allocator, files, resource_budget, errors);
    }
    if (manifest.object.get("networkPolicy")) |network_policy| {
        try validateServerNetworkPolicy(allocator, network_policy, errors);
    }
    try validateServerMigrations(allocator, manifest, files, errors);

    if (findPackageFile(files, "index.html")) |html| {
        try validateServerHtmlPolicy(allocator, html, errors);
        if (hasInteractiveWithoutTestId(html)) try errors.append(allocator, "missing_testid");
    }
    if (findPackageFile(files, "styles.css")) |css| {
        try validateServerCssPolicy(allocator, css, errors);
    }
    if (findPackageFile(files, "app.js")) |js| {
        try validateServerJsPolicy(allocator, manifest, js, errors);
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
    const index_html = findPackageFileContent(package_files, "index.html") orelse "";
    const accessibility_title = try htmlTitleOrFallbackAlloc(allocator, index_html, "");
    defer allocator.free(accessibility_title);
    const accessibility_json = try htmlAccessibilityAuditJsonAlloc(allocator, app_id, index_html, accessibility_title);
    defer allocator.free(accessibility_json);
    const accessibility_ok = jsonStringFieldEquals(allocator, accessibility_json, "status", "pass") catch false;
    const security_json = try std.fmt.allocPrint(
        allocator,
        "{{\"ok\":{},\"signature\":{s},\"contentHashes\":{s},\"accessibility\":{s}}}",
        .{ accessibility_ok, signature_json, content_hashes_json, accessibility_json },
    );
    defer allocator.free(security_json);
    const validation_json = try validationReportAlloc(allocator, &.{});
    defer allocator.free(validation_json);
    const allow_runtime_mismatch = allowRuntimeMismatch(allocator);
    const compatibility_ok = runtimeVersionsCompatible(runtime_version, app_runtime_version) or allow_runtime_mismatch;
    const compatibility_json = try runtimeCompatibilityJsonAlloc(allocator, app_runtime_version, compatibility_ok, allow_runtime_mismatch);
    defer allocator.free(compatibility_json);
    const smoke_test = try evaluateSmokeTestsAlloc(allocator, package_root, app_id);
    defer freeSmokeTestEvaluation(allocator, smoke_test);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const previous_install_id = try activeInstallIdAlloc(allocator, db, app_id);
    defer if (previous_install_id) |previous| allocator.free(previous);
    const existing_data_version = try appDataVersion(db, app_id);
    const requires_approval = try packageAddsPermissions(db, permissions, previous_install_id);
    const activate = activate_requested and !requires_approval and smoke_test.ok and accessibility_ok and compatibility_ok;
    const blocked_by_smoke = !smoke_test.ok;
    const blocked_by_accessibility = !accessibility_ok;
    const blocked_by_compatibility = !compatibility_ok;
    const blocked_by_failure = blocked_by_smoke or blocked_by_accessibility or blocked_by_compatibility;
    const version_status = if (activate) "enabled" else if (blocked_by_failure) "quarantined" else "installed";
    const app_status = if (activate or previous_install_id != null) "enabled" else if (blocked_by_failure) "quarantined" else "disabled";
    const stored_data_version = if (activate or previous_install_id == null) data_version else existing_data_version orelse data_version;
    const report_status = if (blocked_by_failure) "failed" else if (requires_approval) "requires-approval" else "accepted";
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
    } else if (blocked_by_failure) {
        const reason = if (blocked_by_compatibility) "runtime compatibility failed" else if (blocked_by_accessibility) "accessibility audit failed" else "smoke-test failed";
        try insertInstallationEvent(db, allocator, app_id, install_id, "quarantine", previous_install_id, report_id, created_at, "zig-server", reason);
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

fn runtimeCompatibilityJsonAlloc(
    allocator: std.mem.Allocator,
    app_runtime_version: []const u8,
    ok: bool,
    allow_runtime_mismatch: bool,
) ![]u8 {
    const escaped_runtime = try escapeJsonString(allocator, runtime_version);
    defer allocator.free(escaped_runtime);
    const escaped_app_runtime = try escapeJsonString(allocator, app_runtime_version);
    defer allocator.free(escaped_app_runtime);
    if (ok) {
        return std.fmt.allocPrint(
            allocator,
            "{{\"ok\":true,\"runtimeVersion\":\"{s}\",\"appRuntimeVersion\":\"{s}\",\"allowRuntimeMismatch\":{}}}",
            .{ escaped_runtime, escaped_app_runtime, allow_runtime_mismatch },
        );
    }
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":false,\"errorCode\":\"runtime_version_incompatible\",\"runtimeVersion\":\"{s}\",\"appRuntimeVersion\":\"{s}\",\"allowRuntimeMismatch\":{}}}",
        .{ escaped_runtime, escaped_app_runtime, allow_runtime_mismatch },
    );
}

const Semver = struct {
    major: u64,
    minor: u64,
    patch: u64,
};

fn runtimeVersionsCompatible(host_version: []const u8, app_runtime_version: []const u8) bool {
    const host = parseSemver(host_version) orelse return false;
    const app = parseSemver(app_runtime_version) orelse return false;
    return app.major == host.major and app.minor <= host.minor;
}

fn parseSemver(version: []const u8) ?Semver {
    var parts = std.mem.splitScalar(u8, version, '.');
    const major_text = parts.next() orelse return null;
    const minor_text = parts.next() orelse return null;
    const patch_raw = parts.next() orelse return null;
    if (parts.next() != null) return null;
    const patch_end = semverNumberPrefixLen(patch_raw);
    if (patch_end == 0) return null;
    return .{
        .major = std.fmt.parseInt(u64, major_text, 10) catch return null,
        .minor = std.fmt.parseInt(u64, minor_text, 10) catch return null,
        .patch = std.fmt.parseInt(u64, patch_raw[0..patch_end], 10) catch return null,
    };
}

fn semverNumberPrefixLen(value: []const u8) usize {
    var index: usize = 0;
    while (index < value.len and value[index] >= '0' and value[index] <= '9') {
        index += 1;
    }
    if (index == value.len) return index;
    if (value[index] == '-' or value[index] == '+') return index;
    return 0;
}

fn allowRuntimeMismatch(allocator: std.mem.Allocator) bool {
    const args = std.process.argsAlloc(allocator) catch return false;
    defer std.process.argsFree(allocator, args);
    for (args[1..]) |arg| {
        if (std.mem.eql(u8, arg, "--allow-runtime-mismatch")) return true;
        if (std.mem.startsWith(u8, arg, "--allow-runtime-mismatch=")) return true;
    }
    return false;
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

fn uninstallWebappControl(allocator: std.mem.Allocator, app_id: []const u8, confirm: bool, actor: []const u8) ![]u8 {
    if (!confirm) return error.ConfirmationRequired;
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    if (!(try appExists(db, app_id))) return error.AppNotInstalled;
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
    const active_install_id = try activeInstallIdAlloc(allocator, db, app_id);
    defer if (active_install_id) |active| allocator.free(active);

    var snapshot_id: ?[]u8 = null;
    if (active_install_id != null) {
        const snapshot_json = try createRuntimeSnapshotInDb(allocator, db, app_id, "manual", null);
        defer allocator.free(snapshot_json);
        snapshot_id = try snapshotIdFromJsonAlloc(allocator, snapshot_json);
    }
    defer if (snapshot_id) |actual_snapshot_id| allocator.free(actual_snapshot_id);

    const cleared_storage_keys = try int64QueryDb(db, "SELECT COUNT(*) FROM app_storage WHERE app_id = ?", app_id);
    _ = try deleteRowsForApp(db, "DELETE FROM app_storage WHERE app_id = ?", app_id);
    try markAllAppVersionsStatus(db, app_id, "uninstalled");
    try markAppUninstalled(db, app_id);
    if (active_install_id) |active| {
        const details_json = try uninstallDetailsJsonAlloc(allocator, snapshot_id, cleared_storage_keys);
        defer allocator.free(details_json);
        try insertLifecycleInstallationEvent(db, allocator, app_id, active, "uninstall", active, null, actor, created_at, details_json);
    }

    const result_json = try uninstallResultJsonAlloc(allocator, app_id, snapshot_id, cleared_storage_keys);
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn approveWebappUpdateControl(allocator: std.mem.Allocator, app_id: []const u8, install_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const target = try installedVersionByInstallIdAlloc(allocator, db, app_id, install_id);
    const target_version = target orelse return error.InstallNotFound;
    defer freeInstalledVersion(allocator, target_version);
    if (std.mem.eql(u8, target_version.status, "quarantined") or std.mem.eql(u8, target_version.status, "uninstalled")) {
        return error.InstallStatusInvalid;
    }
    const report = try latestInstallReportAlloc(allocator, db, app_id, install_id);
    const report_details = report orelse return error.ApprovalNotRequired;
    defer freeInstallReportDetails(allocator, report_details);
    if (!std.mem.eql(u8, report_details.status, "requires-approval")) return error.ApprovalNotRequired;

    const active = try activeInstallDetailsAlloc(allocator, db, app_id);
    defer if (active) |active_version| freeInstalledVersion(allocator, active_version);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);

    var migration_runs: usize = 0;
    if (active) |active_version| {
        if (!std.mem.eql(u8, active_version.install_id, target_version.install_id)) {
            if (target_version.data_version < active_version.data_version) return error.InvalidMigration;
            if (target_version.data_version > active_version.data_version) {
                const package_files = try packageFilesForInstallAlloc(allocator, db, install_id);
                defer freeOwnedPackageFiles(allocator, package_files);
                try applyPackagedMigrationChainForInstall(allocator, db, app_id, install_id, package_files, active_version.data_version, target_version.data_version, created_at);
                migration_runs = @intCast(target_version.data_version - active_version.data_version);
            }
            try markVersionStatus(db, active_version.install_id, "installed", null);
        }
    }

    try markVersionStatus(db, install_id, "enabled", created_at);
    try approveInstallPermissions(db, install_id, created_at);
    try activateInstalledApp(db, app_id, install_id, target_version.version, target_version.data_version, created_at);
    try updateInstallReportApproved(db, allocator, report_details.report_id, created_at);
    const details_json = try approvalDetailsJsonAlloc(allocator, if (active) |active_version| active_version.install_id else null, migration_runs, created_at);
    defer allocator.free(details_json);
    try insertLifecycleInstallationEvent(db, allocator, app_id, install_id, "activate", if (active) |active_version| active_version.install_id else null, report_details.report_id, "zig-server", created_at, details_json);

    const result_json = try approveResultJsonAlloc(allocator, app_id, install_id, if (active) |active_version| active_version.install_id else null, migration_runs);
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn quarantineWebappControl(allocator: std.mem.Allocator, app_id: []const u8, install_id: ?[]const u8, reason: []const u8) ![]u8 {
    return quarantineWebappPackage(allocator, app_id, install_id, reason, false, "zig-server");
}

fn quarantineWebappPackage(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    install_id: ?[]const u8,
    reason: []const u8,
    restore_previous: bool,
    actor: []const u8,
) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const active_install_id = try activeInstallIdAlloc(allocator, db, app_id);
    defer if (active_install_id) |active| allocator.free(active);
    const target_install_id = if (install_id) |explicit_install| explicit_install else active_install_id orelse return error.AppNotInstalled;
    if (!(try installBelongsToApp(db, app_id, target_install_id))) return error.InstallNotFound;
    const restore_target = if (restore_previous and active_install_id != null and std.mem.eql(u8, active_install_id.?, target_install_id))
        try rollbackTargetAlloc(allocator, db, app_id, target_install_id, null)
    else
        null;
    defer if (restore_target) |version| freeInstalledVersion(allocator, version);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);

    try markVersionStatus(db, target_install_id, "quarantined", null);
    if (restore_target) |version| {
        try markVersionStatus(db, version.install_id, "enabled", created_at);
        try activateInstalledApp(db, app_id, version.install_id, version.version, version.data_version, created_at);
    } else if (active_install_id) |active| {
        if (std.mem.eql(u8, active, target_install_id)) {
            try markAppStatus(db, app_id, "quarantined", created_at);
        }
    }
    const restored_install_id = if (restore_target) |version| version.install_id else null;
    const details_json = try quarantineDetailsJsonAlloc(allocator, reason, restored_install_id);
    defer allocator.free(details_json);
    try insertLifecycleInstallationEvent(db, allocator, app_id, target_install_id, "quarantine", restored_install_id, null, actor, created_at, details_json);
    if (restore_target) |version| {
        const rollback_details = try automaticRollbackDetailsJsonAlloc(allocator, target_install_id);
        defer allocator.free(rollback_details);
        try insertLifecycleInstallationEvent(db, allocator, app_id, version.install_id, "rollback", target_install_id, null, actor, created_at, rollback_details);
    }

    const result_json = try quarantineResultJsonAlloc(allocator, app_id, target_install_id, reason);
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

const InstallReportDetails = struct {
    report_id: []u8,
    status: []u8,
};

fn freeInstallReportDetails(allocator: std.mem.Allocator, report: InstallReportDetails) void {
    allocator.free(report.report_id);
    allocator.free(report.status);
}

fn installedVersionByInstallIdAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8, install_id: []const u8) !?InstalledVersion {
    return installedVersionFromQueryAlloc(
        allocator,
        db,
        "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id = ?",
        app_id,
        install_id,
    );
}

fn latestInstallReportAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8, install_id: []const u8) !?InstallReportDetails {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT report_id, status FROM app_install_reports WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC LIMIT 1",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, install_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const report_id = try allocator.dupe(u8, sqliteColumnText(statement, 0));
    errdefer allocator.free(report_id);
    const status = try allocator.dupe(u8, sqliteColumnText(statement, 1));
    errdefer allocator.free(status);
    return .{ .report_id = report_id, .status = status };
}

fn packageFilesForInstallAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, install_id: []const u8) ![]PackageFile {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT path, COALESCE(content_text, ''), content_hash FROM app_files WHERE install_id = ? ORDER BY path",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, install_id);

    var files: std.ArrayList(PackageFile) = .empty;
    errdefer {
        for (files.items) |file| {
            allocator.free(file.path);
            allocator.free(file.content);
            allocator.free(file.content_hash);
        }
        files.deinit(allocator);
    }
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const path = try allocator.dupe(u8, sqliteColumnText(statement, 0));
        errdefer allocator.free(path);
        const content = try allocator.dupe(u8, sqliteColumnText(statement, 1));
        errdefer allocator.free(content);
        const content_hash = try allocator.dupe(u8, sqliteColumnText(statement, 2));
        errdefer allocator.free(content_hash);
        try files.append(allocator, .{ .path = path, .content = content, .content_hash = content_hash });
    }
    return files.toOwnedSlice(allocator);
}

fn freeOwnedPackageFiles(allocator: std.mem.Allocator, files: []PackageFile) void {
    for (files) |file| {
        allocator.free(file.path);
        allocator.free(file.content);
        allocator.free(file.content_hash);
    }
    allocator.free(files);
}

fn appExists(db: *sqlite.sqlite3, app_id: []const u8) !bool {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT 1 FROM apps WHERE id = ? AND status != 'uninstalled' LIMIT 1", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    return sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW;
}

fn installBelongsToApp(db: *sqlite.sqlite3, app_id: []const u8, install_id: []const u8) !bool {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT 1 FROM app_versions WHERE app_id = ? AND install_id = ? LIMIT 1", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, install_id);
    return sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW;
}

fn markAllAppVersionsStatus(db: *sqlite.sqlite3, app_id: []const u8, status: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "UPDATE app_versions SET status = ? WHERE app_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, status);
    bindText(statement, 2, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn markAppUninstalled(db: *sqlite.sqlite3, app_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE apps SET status = 'uninstalled', active_install_id = NULL, active_version = NULL, updated_at = datetime('now') WHERE id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn markAppStatus(db: *sqlite.sqlite3, app_id: []const u8, status: []const u8, updated_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "UPDATE apps SET status = ?, updated_at = ? WHERE id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, status);
    bindText(statement, 2, updated_at);
    bindText(statement, 3, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn approveInstallPermissions(db: *sqlite.sqlite3, install_id: []const u8, approved_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE app_permissions SET approved = 1, approved_at = ?, reason = 'approved update' WHERE install_id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, approved_at);
    bindText(statement, 2, install_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn updateInstallReportApproved(db: *sqlite.sqlite3, allocator: std.mem.Allocator, report_id: []const u8, approved_at: []const u8) !void {
    const permissions_json = try approvalPermissionsJsonAlloc(allocator, approved_at);
    defer allocator.free(permissions_json);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE app_install_reports SET status = 'accepted', permissions_json = ? WHERE report_id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, permissions_json);
    bindText(statement, 2, report_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertLifecycleInstallationEvent(
    db: *sqlite.sqlite3,
    allocator: std.mem.Allocator,
    app_id: []const u8,
    install_id: []const u8,
    action: []const u8,
    previous_install_id: ?[]const u8,
    report_id: ?[]const u8,
    actor: []const u8,
    created_at: []const u8,
    details_json: []const u8,
) !void {
    const event_id = try randomDbIdAlloc(allocator, db, "install_event_");
    defer allocator.free(event_id);
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
    bindNullableText(statement, 7, report_id);
    bindText(statement, 8, created_at);
    bindText(statement, 9, details_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn uninstallDetailsJsonAlloc(allocator: std.mem.Allocator, snapshot_id: ?[]const u8, cleared_storage_keys: i64) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"snapshotId\":");
    try appendJsonNullableString(allocator, &out, snapshot_id);
    try out.writer.print(",\"clearedStorageKeys\":{d}}}", .{cleared_storage_keys});
    return out.toOwnedSlice();
}

fn uninstallResultJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, snapshot_id: ?[]const u8, cleared_storage_keys: i64) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print("{{\"ok\":true,\"appId\":\"{s}\",\"status\":\"uninstalled\",\"snapshotId\":", .{escaped_app_id});
    try appendJsonNullableString(allocator, &out, snapshot_id);
    try out.writer.print(",\"clearedStorageKeys\":{d}}}", .{cleared_storage_keys});
    return out.toOwnedSlice();
}

fn approvalPermissionsJsonAlloc(allocator: std.mem.Allocator, approved_at: []const u8) ![]u8 {
    const escaped = try escapeJsonString(allocator, approved_at);
    defer allocator.free(escaped);
    return std.fmt.allocPrint(allocator, "{{\"requiresUserApproval\":true,\"approvalGranted\":true,\"approvedAt\":\"{s}\"}}", .{escaped});
}

fn approvalDetailsJsonAlloc(allocator: std.mem.Allocator, previous_install_id: ?[]const u8, migration_runs: usize, approved_at: []const u8) ![]u8 {
    const escaped_approved_at = try escapeJsonString(allocator, approved_at);
    defer allocator.free(escaped_approved_at);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"approved\":true,\"previousInstallId\":");
    try appendJsonNullableString(allocator, &out, previous_install_id);
    try out.writer.print(",\"migrationRuns\":{d},\"approvedAt\":\"{s}\"}}", .{ migration_runs, escaped_approved_at });
    return out.toOwnedSlice();
}

fn approveResultJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, install_id: []const u8, previous_install_id: ?[]const u8, migration_runs: usize) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, install_id);
    defer allocator.free(escaped_install_id);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print("{{\"appId\":\"{s}\",\"installId\":\"{s}\",\"status\":\"enabled\",\"previousInstallId\":", .{ escaped_app_id, escaped_install_id });
    try appendJsonNullableString(allocator, &out, previous_install_id);
    try out.writer.print(",\"migrationRuns\":{d}}}", .{migration_runs});
    return out.toOwnedSlice();
}

fn quarantineDetailsJsonAlloc(allocator: std.mem.Allocator, reason: []const u8, restored_install_id: ?[]const u8) ![]u8 {
    const escaped_reason = try escapeJsonString(allocator, reason);
    defer allocator.free(escaped_reason);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print("{{\"reason\":\"{s}\",\"restoredInstallId\":", .{escaped_reason});
    try appendJsonNullableString(allocator, &out, restored_install_id);
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn automaticRollbackDetailsJsonAlloc(allocator: std.mem.Allocator, quarantined_install_id: []const u8) ![]u8 {
    const escaped_install_id = try escapeJsonString(allocator, quarantined_install_id);
    defer allocator.free(escaped_install_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"reason\":\"automatic rollback after quarantine\",\"quarantinedInstallId\":\"{s}\"}}",
        .{escaped_install_id},
    );
}

fn resourceBudgetDetailsJsonAlloc(allocator: std.mem.Allocator, violation: ResourceBudgetViolation) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, violation.app_id);
    defer allocator.free(escaped_app_id);
    const escaped_budget = try escapeJsonString(allocator, violation.budget);
    defer allocator.free(escaped_budget);
    return std.fmt.allocPrint(
        allocator,
        "{{\"appId\":\"{s}\",\"budget\":\"{s}\",\"current\":{d},\"max\":{d},\"limit\":{d}}}",
        .{ escaped_app_id, escaped_budget, violation.current, violation.max, violation.max },
    );
}

fn quarantineResultJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, install_id: []const u8, reason: []const u8) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, install_id);
    defer allocator.free(escaped_install_id);
    const escaped_reason = try escapeJsonString(allocator, reason);
    defer allocator.free(escaped_reason);
    return std.fmt.allocPrint(allocator, "{{\"appId\":\"{s}\",\"installId\":\"{s}\",\"status\":\"quarantined\",\"reason\":\"{s}\"}}", .{ escaped_app_id, escaped_install_id, escaped_reason });
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

fn runtimeSnapshotControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const manifest_json = try activeManifestJsonAlloc(allocator, app_id);
    defer allocator.free(manifest_json);
    var parsed_manifest = std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{}) catch return error.StorageQueryFailed;
    defer parsed_manifest.deinit();
    const manifest_name = if (parsed_manifest.value == .object)
        valueString(parsed_manifest.value.object.get("name")) orelse app_id
    else
        app_id;

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const active = try activeInstallDetailsAlloc(allocator, db, app_id);
    const active_version = active orelse return error.AppNotInstalled;
    defer freeInstalledVersion(allocator, active_version);

    const package_files = try packageFilesForInstallAlloc(allocator, db, active_version.install_id);
    defer freeOwnedPackageFiles(allocator, package_files);
    const html = findPackageFileContent(package_files, "index.html") orelse "";

    const title = try htmlTitleOrFallbackAlloc(allocator, html, manifest_name);
    defer allocator.free(title);
    const text = try htmlTextAlloc(allocator, html);
    defer allocator.free(text);
    const test_ids = try htmlDataTestIdsJsonAlloc(allocator, html);
    defer allocator.free(test_ids);
    const dom_summary = try htmlDomSummaryJsonAlloc(allocator, html, text);
    defer allocator.free(dom_summary);
    const accessibility_tree = try htmlAccessibilityTreeJsonAlloc(allocator, app_id, html, title);
    defer allocator.free(accessibility_tree);
    const resource_usage = try snapshotResourceUsageJsonAlloc(allocator, db, app_id);
    defer allocator.free(resource_usage);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"installId\":");
    try appendJsonString(allocator, &out, active_version.install_id);
    try out.writer.writeAll(",\"version\":");
    try appendJsonString(allocator, &out, active_version.version);
    try out.writer.writeAll(",\"route\":\"/\",\"title\":");
    try appendJsonString(allocator, &out, title);
    try out.writer.print(",\"testIds\":{s},\"text\":", .{test_ids});
    try appendJsonString(allocator, &out, text);
    try out.writer.print(",\"domSummary\":{s},\"accessibilityTree\":{s},\"errors\":[],\"resourceUsage\":{s}}}", .{ dom_summary, accessibility_tree, resource_usage });
    return out.toOwnedSlice();
}

const RuntimeHtmlPackage = struct {
    install_id: []u8,
    version: []u8,
    html: []u8,
};

fn freeRuntimeHtmlPackage(allocator: std.mem.Allocator, package: RuntimeHtmlPackage) void {
    allocator.free(package.install_id);
    allocator.free(package.version);
    allocator.free(package.html);
}

fn runtimeHtmlPackageAlloc(allocator: std.mem.Allocator, app_id: []const u8) !RuntimeHtmlPackage {
    const manifest_json = try activeManifestJsonAlloc(allocator, app_id);
    defer allocator.free(manifest_json);

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const active = try activeInstallDetailsAlloc(allocator, db, app_id);
    const active_version = active orelse return error.AppNotInstalled;
    errdefer freeInstalledVersion(allocator, active_version);

    const package_files = try packageFilesForInstallAlloc(allocator, db, active_version.install_id);
    defer freeOwnedPackageFiles(allocator, package_files);
    const html = try allocator.dupe(u8, findPackageFileContent(package_files, "index.html") orelse "");
    errdefer allocator.free(html);
    allocator.free(active_version.status);
    return .{
        .install_id = active_version.install_id,
        .version = active_version.version,
        .html = html,
    };
}

fn installedAppJsAlloc(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    return (try installedPackageFileAlloc(allocator, app_id, "app.js")) orelse allocator.dupe(u8, "");
}

fn installedPackageFileAlloc(allocator: std.mem.Allocator, app_id: []const u8, file_path: []const u8) !?[]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const active = try activeInstallDetailsAlloc(allocator, db, app_id);
    const active_version = active orelse return error.AppNotInstalled;
    defer freeInstalledVersion(allocator, active_version);
    const package_files = try packageFilesForInstallAlloc(allocator, db, active_version.install_id);
    defer freeOwnedPackageFiles(allocator, package_files);
    const content = findPackageFileContent(package_files, file_path) orelse return null;
    return @as(?[]u8, try allocator.dupe(u8, content));
}

fn runtimeQueryControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const query = try runtimeQueryLabelAlloc(allocator, args);
    defer allocator.free(query);
    const match_json = try runtimeFirstMatchJsonAlloc(allocator, package.html, args);
    defer if (match_json) |actual| allocator.free(actual);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":");
    try out.writer.writeAll(if (match_json != null) "true" else "false");
    try out.writer.writeAll(",\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"query\":");
    try appendJsonString(allocator, &out, query);
    try out.writer.writeAll(",\"matches\":");
    if (match_json) |actual| {
        try out.writer.print("[{s}]", .{actual});
    } else {
        try out.writer.writeAll("[]");
    }
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn runtimeScreenshotControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const title = try htmlTitleOrFallbackAlloc(allocator, package.html, app_id);
    defer allocator.free(title);
    const text = try htmlTextAlloc(allocator, package.html);
    defer allocator.free(text);
    const text_hash = try sha256PrefixedAlloc(allocator, text);
    defer allocator.free(text_hash);
    const test_ids = try htmlDataTestIdsJsonAlloc(allocator, package.html);
    defer allocator.free(test_ids);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"label\":");
    try appendJsonNullableString(allocator, &out, controlStringArg(args, "label"));
    try out.writer.writeAll(",\"format\":\"static-html-summary\",\"title\":");
    try appendJsonString(allocator, &out, title);
    try out.writer.writeAll(",\"textHash\":");
    try appendJsonString(allocator, &out, text_hash);
    try out.writer.print(",\"testIds\":{s}}}", .{test_ids});
    return out.toOwnedSlice();
}

fn runtimeTargetControl(allocator: std.mem.Allocator, tool: []const u8, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const match_json = try runtimeFirstMatchJsonAlloc(allocator, package.html, args);
    const target = match_json orelse return error.SelectorNotFound;
    defer allocator.free(target);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"tool\":");
    try appendJsonString(allocator, &out, tool);
    try out.writer.print(",\"target\":{s}}}", .{target});
    return out.toOwnedSlice();
}

fn runtimePressKeyControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"key\":");
    try appendJsonNullableString(allocator, &out, controlStringArg(args, "key"));
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn runtimeWaitForControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"kind\":");
    try appendJsonString(allocator, &out, controlStringArg(args, "kind") orelse "idle");
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn assertRuntimeVisibleControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const match_json = try runtimeFirstMatchJsonAlloc(allocator, package.html, args);
    if (match_json) |actual| {
        allocator.free(actual);
    } else {
        return error.SelectorNotFound;
    }
    return allocator.dupe(u8, "{\"ok\":true,\"matches\":1}");
}

fn assertRuntimeTextControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const text = controlStringArg(args, "text") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const html_text = try htmlTextAlloc(allocator, package.html);
    defer allocator.free(html_text);
    if (std.mem.indexOf(u8, html_text, text) == null) return error.TextNotFound;

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"text\":");
    try appendJsonString(allocator, &out, text);
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn runtimeAccessibilitySnapshotControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const title = try htmlTitleOrFallbackAlloc(allocator, package.html, "");
    defer allocator.free(title);
    return htmlAccessibilityTreeJsonAlloc(allocator, app_id, package.html, title);
}

fn runtimeAccessibilityAuditControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const title = try htmlTitleOrFallbackAlloc(allocator, package.html, "");
    defer allocator.free(title);
    return htmlAccessibilityAuditJsonAlloc(allocator, app_id, package.html, title);
}

fn runtimeAssertAccessibilityControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    const rule = controlStringArg(args, "rule");
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const title = try htmlTitleOrFallbackAlloc(allocator, package.html, "");
    defer allocator.free(title);
    const report = try htmlAccessibilityAuditJsonAlloc(allocator, app_id, package.html, title);
    defer allocator.free(report);
    if (try htmlAccessibilityFails(allocator, package.html, title, rule)) return error.AccessibilityFailed;

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"rule\":");
    try appendJsonNullableString(allocator, &out, rule);
    try out.writer.print(",\"report\":{s}}}", .{report});
    return out.toOwnedSlice();
}

fn runtimeRunSmokeTestsControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId") orelse return error.InvalidControlArgs;
    return runSmokeTestsForAppControl(allocator, app_id);
}

fn runSmokeTestsForAppControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const evaluation = try evaluateInstalledSmokeTestsAlloc(allocator, app_id);
    defer freeSmokeTestEvaluation(allocator, evaluation);
    const micro_test_id = try std.fmt.allocPrint(allocator, "smoke:{s}", .{app_id});
    defer allocator.free(micro_test_id);
    const name = try std.fmt.allocPrint(allocator, "{s} bundled smoke tests", .{app_id});
    defer allocator.free(name);
    return recordControlTestRun(allocator, micro_test_id, name, app_id, testRunStatus(evaluation.status), evaluation.spec_json, evaluation.result_json, "zig-server-static-smoke");
}

fn runtimeRunMicrotestControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const spec_json = try controlSpecJsonAlloc(allocator, args, "spec", "microtestPath");
    defer allocator.free(spec_json);
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, spec_json, .{}) catch return error.InvalidControlArgs;
    defer parsed.deinit();
    if (parsed.value != .object) return error.InvalidMicrotest;
    const app_id = firstTargetApp(parsed.value) orelse return error.InvalidMicrotest;
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const app_js = try installedAppJsAlloc(allocator, app_id);
    defer allocator.free(app_js);

    const result_json = try evaluateMicrotestSpecJsonAlloc(allocator, parsed.value, package.html, app_js);
    defer allocator.free(result_json);
    const status = if (std.mem.indexOf(u8, result_json, "\"ok\":true") != null) "passed" else "failed";
    const micro_test_id = valueString(parsed.value.object.get("id")) orelse "microtest";
    return recordControlTestRun(allocator, micro_test_id, micro_test_id, app_id, status, spec_json, result_json, "zig-server-static-microtest");
}

fn platformRunSmokeControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const spec_json = try controlSpecJsonAlloc(allocator, args, "spec", "smokePath");
    defer allocator.free(spec_json);
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, spec_json, .{}) catch return error.InvalidControlArgs;
    defer parsed.deinit();
    if (parsed.value != .object) return error.InvalidControlArgs;
    const smoke_id = valueString(parsed.value.object.get("id")) orelse "platform-smoke";
    const platform = controlStringArg(args, "platform") orelse "zig-server";
    const apps = parsed.value.object.get("apps") orelse return error.InvalidControlArgs;
    if (apps != .array) return error.InvalidControlArgs;

    var failures: std.io.Writer.Allocating = .init(allocator);
    errdefer failures.deinit();
    var app_results: std.io.Writer.Allocating = .init(allocator);
    errdefer app_results.deinit();
    try failures.writer.writeAll("[");
    try app_results.writer.writeAll("[");
    var failure_count: usize = 0;
    var app_count: usize = 0;
    for (apps.array.items) |app_value| {
        const app_id = valueString(app_value) orelse continue;
        const smoke = evaluateInstalledSmokeTestsAlloc(allocator, app_id) catch |err| switch (err) {
            error.AppNotInstalled => {
                if (failure_count > 0) try failures.writer.writeAll(",");
                try failures.writer.writeAll("{\"appId\":");
                try appendJsonString(allocator, &failures, app_id);
                try failures.writer.writeAll(",\"code\":\"app_not_installed\"}");
                failure_count += 1;
                if (app_count > 0) try app_results.writer.writeAll(",");
                try app_results.writer.writeAll("{\"appId\":");
                try appendJsonString(allocator, &app_results, app_id);
                try app_results.writer.writeAll(",\"ok\":false,\"commands\":[]}");
                app_count += 1;
                continue;
            },
            else => return err,
        };
        defer freeSmokeTestEvaluation(allocator, smoke);
        if (!smoke.ok) {
            if (failure_count > 0) try failures.writer.writeAll(",");
            try failures.writer.writeAll("{\"appId\":");
            try appendJsonString(allocator, &failures, app_id);
            try failures.writer.print(",\"code\":\"smoke_failed\",\"result\":{s}}}", .{smoke.result_json});
            failure_count += 1;
        }
        if (app_count > 0) try app_results.writer.writeAll(",");
        try app_results.writer.writeAll("{\"appId\":");
        try appendJsonString(allocator, &app_results, app_id);
        try app_results.writer.print(",\"ok\":{},\"commands\":[{{\"tool\":\"runtime.run_smoke_tests\",\"status\":\"{s}\",\"result\":{s}}}]}}", .{ smoke.ok, smoke.status, smoke.result_json });
        app_count += 1;
    }
    try failures.writer.writeAll("]");
    try app_results.writer.writeAll("]");
    const failures_json = try failures.toOwnedSlice();
    defer allocator.free(failures_json);
    const apps_json = try app_results.toOwnedSlice();
    defer allocator.free(apps_json);

    var result: std.io.Writer.Allocating = .init(allocator);
    errdefer result.deinit();
    try result.writer.writeAll("{\"ok\":");
    try result.writer.writeAll(if (failure_count == 0) "true" else "false");
    try result.writer.writeAll(",\"id\":");
    try appendJsonString(allocator, &result, smoke_id);
    try result.writer.writeAll(",\"platform\":");
    try appendJsonString(allocator, &result, platform);
    try result.writer.print(",\"totalApps\":{d},\"failures\":{s},\"apps\":{s}}}", .{ app_count, failures_json, apps_json });
    const result_json = try result.toOwnedSlice();
    defer allocator.free(result_json);

    const micro_test_id = try std.fmt.allocPrint(allocator, "platform-smoke:{s}:{s}", .{ smoke_id, platform });
    defer allocator.free(micro_test_id);
    const name = try std.fmt.allocPrint(allocator, "{s} ({s})", .{ smoke_id, platform });
    defer allocator.free(name);
    return recordControlTestRun(allocator, micro_test_id, name, null, if (failure_count == 0) "passed" else "failed", spec_json, result_json, "zig-server-static-platform-smoke");
}

fn platformRunRepairLoopControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const args_value = args orelse return error.InvalidControlArgs;
    const package_root = packageRootValue(args_value) orelse return error.InvalidControlArgs;
    const started_at = try serverNowIsoAlloc(allocator);
    defer allocator.free(started_at);
    const trust_level = controlStringArg(args, "trustLevel") orelse "developer";

    var steps: std.io.Writer.Allocating = .init(allocator);
    errdefer steps.deinit();
    try steps.writer.writeAll("[");
    var step_count: usize = 0;
    var final_status: []const u8 = "failed";
    var app_id: ?[]u8 = null;
    defer if (app_id) |actual| allocator.free(actual);
    var tests_run: std.io.Writer.Allocating = .init(allocator);
    errdefer tests_run.deinit();
    try tests_run.writer.writeAll("[");
    var tests_count: usize = 0;
    var snapshots: std.io.Writer.Allocating = .init(allocator);
    errdefer snapshots.deinit();
    try snapshots.writer.writeAll("[");
    var snapshot_count: usize = 0;

    const validation = try validateWebappPackageValue(allocator, package_root);
    defer allocator.free(validation);
    try appendRepairStepJson(allocator, &steps, &step_count, "platform.validate_package", if (jsonOk(validation)) "passed" else "failed", validation);
    if (!jsonOk(validation)) {
        final_status = "failed";
    } else {
        const signed = try signWebappPackage(allocator, package_root, trust_level);
        defer allocator.free(signed);
        try appendRepairStepJson(allocator, &steps, &step_count, "platform.sign_webapp_package", "passed", signed);

        const install = installWebappPackage(allocator, package_root, true, trust_level) catch |err| switch (err) {
            error.InvalidWebappPackage => blk: {
                const failed = try allocator.dupe(u8, "{\"ok\":false,\"status\":\"failed\",\"error\":\"invalid_package\"}");
                break :blk failed;
            },
            error.InvalidMigration => blk: {
                const failed = try allocator.dupe(u8, "{\"ok\":false,\"status\":\"failed\",\"error\":\"invalid_migration\"}");
                break :blk failed;
            },
            else => return err,
        };
        defer allocator.free(install);
        const install_enabled = jsonStringFieldEquals(allocator, install, "status", "enabled") catch false;
        try appendRepairStepJson(allocator, &steps, &step_count, "platform.install_webapp_package", if (install_enabled) "passed" else "failed", install);
        app_id = try jsonStringFieldAlloc(allocator, install, "appId");
        const install_status = try jsonStringFieldAlloc(allocator, install, "status");
        defer if (install_status) |status| allocator.free(status);
        if (install_status) |status| {
            if (std.mem.eql(u8, status, "requires-approval")) {
                final_status = "requires-approval";
            } else if (std.mem.eql(u8, status, "enabled")) {
                final_status = "passed";
            }
        }
        if (app_id) |actual_app_id| {
            if (install_status != null and std.mem.eql(u8, install_status.?, "enabled")) {
                const opened = try openWebappControl(allocator, actual_app_id);
                defer allocator.free(opened);
                try appendRepairStepJson(allocator, &steps, &step_count, "platform.open_webapp", "passed", opened);

                const capabilities = try serverCapabilitiesJson(allocator);
                defer allocator.free(capabilities);
                try appendRepairStepJson(allocator, &steps, &step_count, "runtime.capabilities", "passed", capabilities);

                const runtime_snapshot = try runtimeSnapshotControl(allocator, actual_app_id);
                defer allocator.free(runtime_snapshot);
                try appendRepairStepJson(allocator, &steps, &step_count, "runtime.snapshot", "passed", runtime_snapshot);

                const persisted_snapshot = try createRuntimeSnapshot(allocator, actual_app_id, "post-test", null);
                defer allocator.free(persisted_snapshot);
                try appendRepairStepJson(allocator, &steps, &step_count, "platform.create_snapshot", "passed", persisted_snapshot);
                if (try jsonStringFieldAlloc(allocator, persisted_snapshot, "snapshotId")) |snapshot_id| {
                    defer allocator.free(snapshot_id);
                    if (snapshot_count > 0) try snapshots.writer.writeAll(",");
                    try appendJsonString(allocator, &snapshots, snapshot_id);
                    snapshot_count += 1;
                }

                var smoke_ok = true;
                if (controlBoolArg(args, "runSmokeTests") orelse true) {
                    const smoke = try runSmokeTestsForAppControl(allocator, actual_app_id);
                    defer allocator.free(smoke);
                    smoke_ok = jsonStringFieldEquals(allocator, smoke, "status", "passed") catch false;
                    try appendRepairStepJson(allocator, &steps, &step_count, "runtime.run_smoke_tests", if (smoke_ok) "passed" else "failed", smoke);
                    if (try jsonStringFieldAlloc(allocator, smoke, "microTestId")) |micro_test_id| {
                        defer allocator.free(micro_test_id);
                        if (tests_count > 0) try tests_run.writer.writeAll(",");
                        try appendJsonString(allocator, &tests_run, micro_test_id);
                        tests_count += 1;
                    }
                }

                const accessibility = try htmlAccessibilityAuditForAppAlloc(allocator, actual_app_id);
                defer allocator.free(accessibility);
                const accessibility_ok = jsonStringFieldEquals(allocator, accessibility, "status", "pass") catch false;
                try appendRepairStepJson(allocator, &steps, &step_count, "runtime.run_accessibility_audit", if (accessibility_ok) "passed" else "failed", accessibility);

                const resource = try runtimeResourceUsageControl(allocator, actual_app_id);
                defer allocator.free(resource);
                try appendRepairStepJson(allocator, &steps, &step_count, "runtime.resource_usage", "passed", resource);

                const report = try queryInstallReportRowsJson(allocator, actual_app_id, null);
                defer allocator.free(report);
                try appendRepairStepJson(allocator, &steps, &step_count, "platform.install_report", "passed", report);

                final_status = if (smoke_ok and accessibility_ok) "passed" else "failed";
            }
        }
    }

    try steps.writer.writeAll("]");
    const steps_json = try steps.toOwnedSlice();
    defer allocator.free(steps_json);
    try tests_run.writer.writeAll("]");
    const tests_json = try tests_run.toOwnedSlice();
    defer allocator.free(tests_json);
    try snapshots.writer.writeAll("]");
    const snapshots_json = try snapshots.toOwnedSlice();
    defer allocator.free(snapshots_json);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":");
    try out.writer.writeAll(if (std.mem.eql(u8, final_status, "passed")) "true" else "false");
    try out.writer.writeAll(",\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.writeAll(",\"startedAt\":");
    try appendJsonString(allocator, &out, started_at);
    try out.writer.writeAll(",\"attempts\":1,\"finalStatus\":");
    try appendJsonString(allocator, &out, final_status);
    try out.writer.print(",\"changedFiles\":[],\"testsRun\":{s},\"snapshots\":{s},\"remainingWarnings\":[],\"attemptReports\":[{{\"index\":1,\"status\":\"{s}\",\"steps\":{s},\"diagnostics\":{{}}}}]}}", .{ tests_json, snapshots_json, final_status, steps_json });
    return out.toOwnedSlice();
}

fn htmlAccessibilityAuditForAppAlloc(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const title = try htmlTitleOrFallbackAlloc(allocator, package.html, "");
    defer allocator.free(title);
    return htmlAccessibilityAuditJsonAlloc(allocator, app_id, package.html, title);
}

fn appendRepairStepJson(
    allocator: std.mem.Allocator,
    out: *std.io.Writer.Allocating,
    count: *usize,
    tool: []const u8,
    status: []const u8,
    result_json: []const u8,
) !void {
    if (count.* > 0) try out.writer.writeAll(",");
    try out.writer.writeAll("{\"tool\":");
    try appendJsonString(allocator, out, tool);
    try out.writer.writeAll(",\"status\":");
    try appendJsonString(allocator, out, status);
    try out.writer.print(",\"result\":{s}}}", .{result_json});
    count.* += 1;
}

fn jsonStringFieldAlloc(allocator: std.mem.Allocator, json: []const u8, field: []const u8) !?[]u8 {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, json, .{}) catch return null;
    defer parsed.deinit();
    if (parsed.value != .object) return null;
    const value = valueString(parsed.value.object.get(field)) orelse return null;
    return @as(?[]u8, try allocator.dupe(u8, value));
}

fn jsonStringFieldEquals(allocator: std.mem.Allocator, json: []const u8, field: []const u8, expected: []const u8) !bool {
    const actual = try jsonStringFieldAlloc(allocator, json, field);
    defer if (actual) |value| allocator.free(value);
    if (actual) |value| return std.mem.eql(u8, value, expected);
    return false;
}

fn jsonOk(json: []const u8) bool {
    return std.mem.indexOf(u8, json, "\"ok\":true") != null;
}

fn controlSpecJsonAlloc(allocator: std.mem.Allocator, args: ?std.json.Value, spec_key: []const u8, path_key: []const u8) ![]u8 {
    const value = args orelse return error.InvalidControlArgs;
    if (value != .object) return error.InvalidControlArgs;
    if (value.object.get(spec_key)) |spec| return jsonValueAlloc(allocator, spec);
    const path = controlStringArg(args, path_key) orelse return error.InvalidControlArgs;
    if (path.len == 0 or std.fs.path.isAbsolute(path) or containsAny(path, &.{ "..", "\\", "//" })) return error.InvalidControlArgs;
    const file = std.fs.cwd().openFile(path, .{}) catch return error.InvalidControlArgs;
    defer file.close();
    return file.readToEndAlloc(allocator, max_request_bytes);
}

fn firstTargetApp(spec: std.json.Value) ?[]const u8 {
    if (spec != .object) return null;
    const target_apps = spec.object.get("targetApps") orelse return null;
    if (target_apps != .array or target_apps.array.items.len == 0) return null;
    return valueString(target_apps.array.items[0]);
}

fn evaluateMicrotestSpecJsonAlloc(allocator: std.mem.Allocator, spec: std.json.Value, html: []const u8, app_js: []const u8) ![]u8 {
    const microtest_id = valueString(spec.object.get("id")) orelse "microtest";
    var failures: std.io.Writer.Allocating = .init(allocator);
    errdefer failures.deinit();
    try failures.writer.writeAll("[");
    var failure_count: usize = 0;
    var total_steps: usize = 0;
    var dynamic_text: std.ArrayList([]const u8) = .empty;
    defer dynamic_text.deinit(allocator);
    try evaluateMicrotestPhase(allocator, spec.object.get("setup"), html, app_js, &dynamic_text, &failures, &failure_count, &total_steps);
    try evaluateMicrotestPhase(allocator, spec.object.get("steps"), html, app_js, &dynamic_text, &failures, &failure_count, &total_steps);
    try evaluateMicrotestPhase(allocator, spec.object.get("teardown"), html, app_js, &dynamic_text, &failures, &failure_count, &total_steps);
    try failures.writer.writeAll("]");
    const failures_json = try failures.toOwnedSlice();
    defer allocator.free(failures_json);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":");
    try out.writer.writeAll(if (failure_count == 0) "true" else "false");
    try out.writer.writeAll(",\"id\":");
    try appendJsonString(allocator, &out, microtest_id);
    try out.writer.print(",\"totalSteps\":{d},\"failures\":{s},\"setup\":{{\"ok\":true,\"commands\":[]}},\"teardown\":{{\"ok\":true,\"commands\":[]}}}}", .{ total_steps, failures_json });
    return out.toOwnedSlice();
}

fn evaluateMicrotestPhase(
    allocator: std.mem.Allocator,
    phase: ?std.json.Value,
    html: []const u8,
    app_js: []const u8,
    dynamic_text: *std.ArrayList([]const u8),
    failures: *std.io.Writer.Allocating,
    failure_count: *usize,
    total_steps: *usize,
) !void {
    const steps = phase orelse return;
    if (steps != .array) return;
    for (steps.array.items) |step| {
        if (step != .object) continue;
        total_steps.* += 1;
        const tool = valueString(step.object.get("tool")) orelse "";
        const args = step.object.get("args") orelse continue;
        const args_object = if (args == .object) args.object else continue;
        if ((std.mem.eql(u8, tool, "runtime.click") or std.mem.eql(u8, tool, "runtime.type") or std.mem.eql(u8, tool, "runtime.set_value") or std.mem.eql(u8, tool, "runtime.assert_visible"))) {
            if (valueString(args_object.get("testId"))) |test_id| {
                if (!try htmlAttrValueExists(allocator, html, "data-testid", test_id)) {
                    try appendMicroFailure(allocator, failures, failure_count, tool, "selector.not_found", "testId", test_id);
                }
            }
        }
        if (std.mem.eql(u8, tool, "runtime.type")) {
            if (valueString(args_object.get("text"))) |text| try dynamic_text.append(allocator, text);
        }
        if (std.mem.eql(u8, tool, "runtime.set_value")) {
            if (valueString(args_object.get("value"))) |text| try dynamic_text.append(allocator, text);
        }
        if ((std.mem.eql(u8, tool, "runtime.assert_text") or std.mem.eql(u8, tool, "runtime.assert_visible"))) {
            if (valueString(args_object.get("text"))) |text| {
                if (!textCanAppear(html, dynamic_text.items, text)) {
                    try appendMicroFailure(allocator, failures, failure_count, tool, "text.not_found", "text", text);
                }
            }
        }
        if (std.mem.eql(u8, tool, "runtime.assert_bridge_call")) {
            if (valueString(args_object.get("method"))) |method| {
                if (!try bridgeMethodReferenced(allocator, app_js, method)) {
                    try appendMicroFailure(allocator, failures, failure_count, tool, "bridge.call_missing", "method", method);
                }
            }
        }
        if (std.mem.eql(u8, tool, "runtime.replay_events") and !try bridgeMethodReferenced(allocator, app_js, "core.step")) {
            try appendMicroFailure(allocator, failures, failure_count, tool, "core.action_missing", "method", "core.step");
        }
    }
}

fn appendMicroFailure(
    allocator: std.mem.Allocator,
    out: *std.io.Writer.Allocating,
    count: *usize,
    tool: []const u8,
    code: []const u8,
    detail_key: []const u8,
    detail_value: []const u8,
) !void {
    if (count.* > 0) try out.writer.writeAll(",");
    try out.writer.writeAll("{\"tool\":");
    try appendJsonString(allocator, out, tool);
    try out.writer.writeAll(",\"code\":");
    try appendJsonString(allocator, out, code);
    try out.writer.writeAll(",");
    try appendJsonString(allocator, out, detail_key);
    try out.writer.writeAll(":");
    try appendJsonString(allocator, out, detail_value);
    try out.writer.writeAll("}");
    count.* += 1;
}

fn recordControlTestRun(
    allocator: std.mem.Allocator,
    micro_test_id: []const u8,
    name: []const u8,
    app_id: ?[]const u8,
    status: []const u8,
    spec_json: []const u8,
    result_json: []const u8,
    runner: []const u8,
) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
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
    bindNullableText(micro_statement, 2, app_id);
    bindText(micro_statement, 3, name);
    bindText(micro_statement, 4, spec_json);
    bindText(micro_statement, 5, created_at);
    bindText(micro_statement, 6, created_at);
    if (sqlite.sqlite3_step(micro_statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;

    const test_run_id = try randomDbIdAlloc(allocator, db, "testrun_");
    defer allocator.free(test_run_id);
    const diagnostics = try std.fmt.allocPrint(allocator, "{{\"runner\":\"{s}\"}}", .{runner});
    defer allocator.free(diagnostics);
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
    bindNullableText(run_statement, 3, app_id);
    bindText(run_statement, 4, status);
    bindText(run_statement, 5, created_at);
    bindText(run_statement, 6, created_at);
    bindText(run_statement, 7, result_json);
    bindText(run_statement, 8, diagnostics);
    if (sqlite.sqlite3_step(run_statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"testRunId\":");
    try appendJsonString(allocator, &out, test_run_id);
    try out.writer.writeAll(",\"microTestId\":");
    try appendJsonString(allocator, &out, micro_test_id);
    try out.writer.writeAll(",\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.writeAll(",\"status\":");
    try appendJsonString(allocator, &out, status);
    try out.writer.print(",\"result\":{s}}}", .{result_json});
    return out.toOwnedSlice();
}

fn testRunStatus(status: []const u8) []const u8 {
    if (std.mem.eql(u8, status, "passed") or std.mem.eql(u8, status, "failed")) return status;
    if (std.mem.eql(u8, status, "not-run")) return "skipped";
    return "error";
}

fn runtimeQueryLabelAlloc(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    if (controlStringArg(args, "testId")) |test_id| {
        return std.fmt.allocPrint(allocator, "[data-testid=\"{s}\"]", .{test_id});
    }
    if (controlStringArg(args, "selector")) |selector| return allocator.dupe(u8, selector);
    if (controlStringArg(args, "text")) |text| return allocator.dupe(u8, text);
    return allocator.dupe(u8, "");
}

fn runtimeFirstMatchJsonAlloc(allocator: std.mem.Allocator, html: []const u8, args: ?std.json.Value) !?[]u8 {
    if (controlStringArg(args, "testId")) |test_id| {
        const tag = try htmlTagForAttributeAlloc(allocator, html, "data-testid", test_id);
        if (tag) |actual_tag| {
            defer allocator.free(actual_tag);
            return @as(?[]u8, try runtimeMatchJsonAlloc(allocator, "testId", test_id, actual_tag));
        }
        return null;
    }
    if (controlStringArg(args, "selector")) |selector| {
        if (std.mem.startsWith(u8, selector, "#")) {
            const tag = try htmlTagForAttributeAlloc(allocator, html, "id", selector[1..]);
            if (tag) |actual_tag| {
                defer allocator.free(actual_tag);
                return @as(?[]u8, try runtimeMatchJsonAlloc(allocator, "selector", selector, actual_tag));
            }
            return null;
        }
        if (selectorDataTestId(selector)) |test_id| {
            const tag = try htmlTagForAttributeAlloc(allocator, html, "data-testid", test_id);
            if (tag) |actual_tag| {
                defer allocator.free(actual_tag);
                return @as(?[]u8, try runtimeMatchJsonAlloc(allocator, "selector", selector, actual_tag));
            }
            return null;
        }
        if (isSimpleHtmlSelector(selector) and htmlTagExists(html, selector)) {
            return @as(?[]u8, try runtimeMatchJsonAlloc(allocator, "selector", selector, selector));
        }
    }
    if (controlStringArg(args, "text")) |text| {
        const html_text = try htmlTextAlloc(allocator, html);
        defer allocator.free(html_text);
        if (std.mem.indexOf(u8, html_text, text) != null) {
            return @as(?[]u8, try runtimeMatchJsonAlloc(allocator, "text", text, null));
        }
    }
    return null;
}

fn runtimeMatchJsonAlloc(allocator: std.mem.Allocator, kind: []const u8, value: []const u8, tag: ?[]const u8) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"kind\":");
    try appendJsonString(allocator, &out, kind);
    try out.writer.writeAll(",\"value\":");
    try appendJsonString(allocator, &out, value);
    if (tag) |actual_tag| {
        try out.writer.writeAll(",\"tag\":");
        try appendJsonString(allocator, &out, actual_tag);
    }
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn htmlTagForAttributeAlloc(allocator: std.mem.Allocator, html: []const u8, attr: []const u8, value: []const u8) !?[]u8 {
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |tag_start| {
        index = tag_start + 1;
        if (tag_start + 1 >= html.len) continue;
        const first = html[tag_start + 1];
        if (first == '/' or first == '!' or first == '?') continue;
        const name_start = tag_start + 1;
        var name_end = name_start;
        while (name_end < html.len and htmlNameChar(html[name_end])) : (name_end += 1) {}
        if (name_end == name_start) continue;
        const tag_end = std.mem.indexOfScalarPos(u8, html, name_end, '>') orelse return null;
        const tag_source = html[tag_start..tag_end];
        if (try htmlAttrValueExists(allocator, tag_source, attr, value)) {
            return @as(?[]u8, try allocator.dupe(u8, html[name_start..name_end]));
        }
        index = tag_end + 1;
    }
    return null;
}

fn selectorDataTestId(selector: []const u8) ?[]const u8 {
    const start = std.mem.indexOf(u8, selector, "data-testid=") orelse return null;
    const value_start = start + "data-testid=".len;
    if (value_start >= selector.len or (selector[value_start] != '"' and selector[value_start] != '\'')) return null;
    const quote = selector[value_start];
    const actual_start = value_start + 1;
    const actual_end = std.mem.indexOfScalarPos(u8, selector, actual_start, quote) orelse return null;
    return selector[actual_start..actual_end];
}

fn isSimpleHtmlSelector(selector: []const u8) bool {
    if (selector.len == 0 or !std.ascii.isAlphabetic(selector[0])) return false;
    for (selector[1..]) |char| {
        if (!std.ascii.isAlphanumeric(char) and char != '-') return false;
    }
    return true;
}

fn htmlTagExists(html: []const u8, tag: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |tag_start| {
        index = tag_start + 1;
        if (tag_start + 1 >= html.len or html[tag_start + 1] == '/') continue;
        const name_start = tag_start + 1;
        var name_end = name_start;
        while (name_end < html.len and htmlNameChar(html[name_end])) : (name_end += 1) {}
        if (name_end == name_start) continue;
        if (std.ascii.eqlIgnoreCase(html[name_start..name_end], tag)) return true;
    }
    return false;
}

fn htmlNameChar(char: u8) bool {
    return std.ascii.isAlphanumeric(char) or char == '-';
}

fn htmlTitleOrFallbackAlloc(allocator: std.mem.Allocator, html: []const u8, fallback: []const u8) ![]u8 {
    const open = std.mem.indexOf(u8, html, "<title>") orelse return allocator.dupe(u8, fallback);
    const start = open + "<title>".len;
    const close = std.mem.indexOfPos(u8, html, start, "</title>") orelse return allocator.dupe(u8, fallback);
    const title = try htmlTextAlloc(allocator, html[start..close]);
    if (title.len > 0) return title;
    allocator.free(title);
    return allocator.dupe(u8, fallback);
}

fn htmlTextAlloc(allocator: std.mem.Allocator, html: []const u8) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    var in_tag = false;
    var pending_space = false;
    var wrote_any = false;
    for (html) |char| {
        if (char == '<') {
            in_tag = true;
            pending_space = wrote_any;
            continue;
        }
        if (in_tag) {
            if (char == '>') in_tag = false;
            continue;
        }
        if (std.ascii.isWhitespace(char)) {
            pending_space = wrote_any;
            continue;
        }
        if (pending_space) {
            try out.writer.writeByte(' ');
            pending_space = false;
        }
        try out.writer.writeByte(char);
        wrote_any = true;
    }
    return out.toOwnedSlice();
}

fn htmlDataTestIdsJsonAlloc(allocator: std.mem.Allocator, html: []const u8) ![]u8 {
    var ids: std.ArrayList([]const u8) = .empty;
    defer ids.deinit(allocator);
    try collectHtmlDataTestIds(allocator, html, &ids);
    std.mem.sort([]const u8, ids.items, {}, stringSliceLessThan);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    for (ids.items, 0..) |id_value, index| {
        if (index > 0) try out.writer.writeAll(",");
        try appendJsonString(allocator, &out, id_value);
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn collectHtmlDataTestIds(allocator: std.mem.Allocator, html: []const u8, ids: *std.ArrayList([]const u8)) !void {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, html, index, "data-testid")) |attr_start| {
        index = attr_start + "data-testid".len;
        var cursor = index;
        while (cursor < html.len and htmlSpace(html[cursor])) : (cursor += 1) {}
        if (cursor >= html.len or html[cursor] != '=') continue;
        cursor += 1;
        while (cursor < html.len and htmlSpace(html[cursor])) : (cursor += 1) {}
        if (cursor >= html.len or (html[cursor] != '"' and html[cursor] != '\'')) continue;
        const quote = html[cursor];
        const value_start = cursor + 1;
        const value_end = std.mem.indexOfScalarPos(u8, html, value_start, quote) orelse break;
        try ids.append(allocator, html[value_start..value_end]);
        index = value_end + 1;
    }
}

fn htmlDomSummaryJsonAlloc(allocator: std.mem.Allocator, html: []const u8, text: []const u8) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"htmlBytes\":{d},\"textBytes\":{d},\"tagCount\":{d},\"testIdCount\":{d}}}",
        .{ html.len, text.len, countByte(html, '<'), htmlDataTestIdCount(html) },
    );
}

fn htmlAccessibilityTreeJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, html: []const u8, title: []const u8) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"title\":");
    try appendJsonString(allocator, &out, title);
    try out.writer.writeAll(",\"landmarks\":");
    if (std.mem.indexOf(u8, html, "<main") != null) {
        try out.writer.writeAll("[{\"role\":\"main\",\"selector\":\"main\"}]");
    } else {
        try out.writer.writeAll("[]");
    }
    try out.writer.writeAll(",\"headings\":");
    try appendHtmlHeadingsJson(allocator, &out, html);
    try out.writer.writeAll(",\"controls\":");
    try appendHtmlControlsJson(allocator, &out, html);
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn htmlAccessibilityAuditJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, html: []const u8, title: []const u8) ![]u8 {
    const checked_at = try serverNowIsoAlloc(allocator);
    defer allocator.free(checked_at);
    const unlabeled_selector = try firstUnlabeledControlSelectorAlloc(allocator, html);
    defer if (unlabeled_selector) |selector| allocator.free(selector);
    const title_ok = title.len > 0;
    const main_ok = std.mem.indexOf(u8, html, "<main") != null;
    const h1_ok = htmlHasHeadingLevel(html, '1');
    const controls_ok = unlabeled_selector == null;
    const status = if (title_ok and main_ok and h1_ok and controls_ok) "pass" else "fail";

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonString(allocator, &out, app_id);
    try out.writer.writeAll(",\"checkedAt\":");
    try appendJsonString(allocator, &out, checked_at);
    try out.writer.writeAll(",\"status\":");
    try appendJsonString(allocator, &out, status);
    try out.writer.writeAll(",\"checks\":[");
    try appendAccessibilityCheckJson(allocator, &out, "document_title", title_ok, "Document must include a non-empty <title>.", null);
    try out.writer.writeAll(",");
    try appendAccessibilityCheckJson(allocator, &out, "main_landmark", main_ok, "Page must include a <main> landmark.", null);
    try out.writer.writeAll(",");
    try appendAccessibilityCheckJson(allocator, &out, "screen_title", h1_ok, "Page must include an h1 screen title.", null);
    try out.writer.writeAll(",");
    try appendAccessibilityCheckJson(allocator, &out, "no_unlabeled_controls", controls_ok, "Every interactive control must have an accessible name.", unlabeled_selector);
    try out.writer.writeAll("]}");
    return out.toOwnedSlice();
}

fn htmlAccessibilityFails(allocator: std.mem.Allocator, html: []const u8, title: []const u8, rule: ?[]const u8) !bool {
    const unlabeled_selector = try firstUnlabeledControlSelectorAlloc(allocator, html);
    defer if (unlabeled_selector) |selector| allocator.free(selector);
    const all_failed = [_]struct { id: []const u8, failed: bool }{
        .{ .id = "document_title", .failed = title.len == 0 },
        .{ .id = "main_landmark", .failed = std.mem.indexOf(u8, html, "<main") == null },
        .{ .id = "screen_title", .failed = !htmlHasHeadingLevel(html, '1') },
        .{ .id = "no_unlabeled_controls", .failed = unlabeled_selector != null },
    };
    for (all_failed) |check| {
        if (rule) |actual_rule| {
            if (!std.mem.eql(u8, actual_rule, check.id)) continue;
        }
        if (check.failed) return true;
    }
    return false;
}

fn appendAccessibilityCheckJson(
    allocator: std.mem.Allocator,
    out: *std.io.Writer.Allocating,
    id: []const u8,
    ok: bool,
    message: []const u8,
    selector: ?[]const u8,
) !void {
    try out.writer.writeAll("{\"id\":");
    try appendJsonString(allocator, out, id);
    try out.writer.writeAll(",\"status\":");
    try appendJsonString(allocator, out, if (ok) "pass" else "fail");
    try out.writer.writeAll(",\"message\":");
    try appendJsonString(allocator, out, message);
    if (selector) |actual_selector| {
        try out.writer.writeAll(",\"selector\":");
        try appendJsonString(allocator, out, actual_selector);
    }
    try out.writer.writeAll("}");
}

fn appendHtmlHeadingsJson(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, html: []const u8) !void {
    try out.writer.writeAll("[");
    var count: usize = 0;
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |tag_start| {
        index = tag_start + 1;
        if (tag_start + 3 >= html.len) continue;
        if (html[tag_start + 1] != 'h') continue;
        const level_char = html[tag_start + 2];
        if (level_char < '1' or level_char > '6') continue;
        const after_level = tag_start + 3;
        if (after_level >= html.len or !htmlTagBoundary(html[after_level])) continue;
        const open_end = std.mem.indexOfScalarPos(u8, html, after_level, '>') orelse continue;
        const close_tag = try std.fmt.allocPrint(allocator, "</h{c}>", .{level_char});
        defer allocator.free(close_tag);
        const close_start = std.mem.indexOfPos(u8, html, open_end + 1, close_tag) orelse continue;
        const name = try htmlTextAlloc(allocator, html[open_end + 1 .. close_start]);
        defer allocator.free(name);
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.print("{{\"level\":{c},\"name\":", .{level_char});
        try appendJsonString(allocator, out, name);
        try out.writer.writeAll("}");
        count += 1;
        index = close_start + close_tag.len;
    }
    try out.writer.writeAll("]");
}

fn appendHtmlControlsJson(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, html: []const u8) !void {
    try out.writer.writeAll("[");
    var count: usize = 0;
    try appendPairedControlsJson(allocator, out, html, &count, "button");
    try appendPairedControlsJson(allocator, out, html, &count, "select");
    try appendPairedControlsJson(allocator, out, html, &count, "textarea");
    try appendPairedControlsJson(allocator, out, html, &count, "a");
    try appendInputControlsJson(allocator, out, html, &count);
    try out.writer.writeAll("]");
}

fn appendPairedControlsJson(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, html: []const u8, count: *usize, tag: []const u8) !void {
    var index: usize = 0;
    while (findOpeningTag(html, tag, index)) |start| {
        const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return;
        const close_tag = try std.fmt.allocPrint(allocator, "</{s}>", .{tag});
        defer allocator.free(close_tag);
        const close_start = std.mem.indexOfPos(u8, html, open_end + 1, close_tag) orelse {
            index = open_end + 1;
            continue;
        };
        if (count.* > 0) try out.writer.writeAll(",");
        try appendControlRecordJson(allocator, out, html, tag, html[start + tag.len + 1 .. open_end], html[open_end + 1 .. close_start], start);
        count.* += 1;
        index = close_start + close_tag.len;
    }
}

fn appendInputControlsJson(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, html: []const u8, count: *usize) !void {
    var index: usize = 0;
    while (findOpeningTag(html, "input", index)) |start| {
        const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return;
        const attrs = html[start + "<input".len .. open_end];
        const input_type = try htmlAttrValueAlloc(allocator, attrs, "type");
        defer if (input_type) |actual| allocator.free(actual);
        if (input_type) |actual| {
            if (std.ascii.eqlIgnoreCase(actual, "hidden")) {
                index = open_end + 1;
                continue;
            }
        }
        if (count.* > 0) try out.writer.writeAll(",");
        try appendControlRecordJson(allocator, out, html, "input", attrs, "", start);
        count.* += 1;
        index = open_end + 1;
    }
}

fn appendControlRecordJson(
    allocator: std.mem.Allocator,
    out: *std.io.Writer.Allocating,
    html: []const u8,
    tag: []const u8,
    attrs: []const u8,
    inner_html: []const u8,
    tag_start: usize,
) !void {
    const test_id = try htmlAttrValueAlloc(allocator, attrs, "data-testid");
    defer if (test_id) |actual| allocator.free(actual);
    const id = try htmlAttrValueAlloc(allocator, attrs, "id");
    defer if (id) |actual| allocator.free(actual);
    const input_type = try htmlAttrValueAlloc(allocator, attrs, "type");
    defer if (input_type) |actual| allocator.free(actual);
    const selector = try controlSelectorAlloc(allocator, tag, test_id, id);
    defer allocator.free(selector);
    const name = try controlAccessibleNameAlloc(allocator, html, tag, attrs, inner_html, tag_start, id);
    defer allocator.free(name);

    try out.writer.writeAll("{\"tag\":");
    try appendJsonString(allocator, out, tag);
    try out.writer.writeAll(",\"type\":");
    try appendJsonNullableString(allocator, out, input_type);
    try out.writer.writeAll(",\"testId\":");
    try appendJsonString(allocator, out, test_id orelse "");
    try out.writer.writeAll(",\"selector\":");
    try appendJsonString(allocator, out, selector);
    try out.writer.writeAll(",\"name\":");
    try appendJsonString(allocator, out, name);
    try out.writer.writeAll("}");
}

fn firstUnlabeledControlSelectorAlloc(allocator: std.mem.Allocator, html: []const u8) !?[]u8 {
    const tags = [_][]const u8{ "button", "select", "textarea", "a", "input" };
    var best_start: ?usize = null;
    var best_tag: []const u8 = "";
    for (tags) |tag| {
        if (findOpeningTag(html, tag, 0)) |start| {
            if (best_start == null or start < best_start.?) {
                best_start = start;
                best_tag = tag;
            }
        }
    }
    var cursor: usize = 0;
    while (best_start) |start| {
        const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return null;
        const attrs_start = start + best_tag.len + 2;
        const attrs = html[@min(attrs_start, open_end)..open_end];
        if (std.mem.eql(u8, best_tag, "input")) {
            const input_type = try htmlAttrValueAlloc(allocator, attrs, "type");
            defer if (input_type) |actual| allocator.free(actual);
            if (input_type) |actual| {
                if (std.ascii.eqlIgnoreCase(actual, "hidden")) {
                    cursor = open_end + 1;
                    best_start = nextControlStart(html, tags[0..], cursor, &best_tag);
                    continue;
                }
            }
        }
        const close_start = if (std.mem.eql(u8, best_tag, "input")) open_end else blk: {
            const close_tag = try std.fmt.allocPrint(allocator, "</{s}>", .{best_tag});
            defer allocator.free(close_tag);
            break :blk std.mem.indexOfPos(u8, html, open_end + 1, close_tag) orelse open_end;
        };
        const inner = if (close_start > open_end) html[open_end + 1 .. close_start] else "";
        const id = try htmlAttrValueAlloc(allocator, attrs, "id");
        defer if (id) |actual| allocator.free(actual);
        const name = try controlAccessibleNameAlloc(allocator, html, best_tag, attrs, inner, start, id);
        defer allocator.free(name);
        if (name.len == 0) {
            const test_id = try htmlAttrValueAlloc(allocator, attrs, "data-testid");
            defer if (test_id) |actual| allocator.free(actual);
            return try controlSelectorAlloc(allocator, best_tag, test_id, id);
        }
        cursor = open_end + 1;
        best_start = nextControlStart(html, tags[0..], cursor, &best_tag);
    }
    return null;
}

fn nextControlStart(html: []const u8, tags: []const []const u8, start: usize, found_tag: *[]const u8) ?usize {
    var best: ?usize = null;
    var best_tag: []const u8 = "";
    for (tags) |tag| {
        if (findOpeningTag(html, tag, start)) |candidate| {
            if (best == null or candidate < best.?) {
                best = candidate;
                best_tag = tag;
            }
        }
    }
    if (best) |_| found_tag.* = best_tag;
    return best;
}

fn controlAccessibleNameAlloc(
    allocator: std.mem.Allocator,
    html: []const u8,
    tag: []const u8,
    attrs: []const u8,
    inner_html: []const u8,
    tag_start: usize,
    id: ?[]const u8,
) ![]u8 {
    const aria = try htmlAttrValueAlloc(allocator, attrs, "aria-label");
    defer if (aria) |actual| allocator.free(actual);
    if (aria) |actual| {
        const trimmed = std.mem.trim(u8, actual, " \t\r\n");
        if (trimmed.len > 0) return allocator.dupe(u8, trimmed);
    }
    const title = try htmlAttrValueAlloc(allocator, attrs, "title");
    defer if (title) |actual| allocator.free(actual);
    if (title) |actual| {
        const trimmed = std.mem.trim(u8, actual, " \t\r\n");
        if (trimmed.len > 0) return allocator.dupe(u8, trimmed);
    }
    if (std.mem.eql(u8, tag, "button") or std.mem.eql(u8, tag, "a")) {
        const inner_text = try htmlTextAlloc(allocator, inner_html);
        defer allocator.free(inner_text);
        const trimmed = std.mem.trim(u8, inner_text, " \t\r\n");
        if (trimmed.len > 0) return allocator.dupe(u8, trimmed);
    }
    if (id) |actual_id| {
        const explicit = try explicitLabelForIdAlloc(allocator, html, actual_id);
        if (explicit) |label| return label;
    }
    if (try wrappingLabelForControlAlloc(allocator, html, tag_start)) |wrapped| return wrapped;
    return allocator.dupe(u8, "");
}

fn controlSelectorAlloc(allocator: std.mem.Allocator, tag: []const u8, test_id: ?[]const u8, id: ?[]const u8) ![]u8 {
    if (test_id) |actual_test_id| return std.fmt.allocPrint(allocator, "[data-testid=\"{s}\"]", .{actual_test_id});
    if (id) |actual_id| return std.fmt.allocPrint(allocator, "#{s}", .{actual_id});
    return allocator.dupe(u8, tag);
}

fn htmlAttrValueAlloc(allocator: std.mem.Allocator, attrs: []const u8, attr: []const u8) !?[]u8 {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, attrs, index, attr)) |attr_start| {
        if (attr_start > 0 and htmlNameChar(attrs[attr_start - 1])) {
            index = attr_start + attr.len;
            continue;
        }
        var cursor = attr_start + attr.len;
        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor >= attrs.len or attrs[cursor] != '=') {
            index = cursor;
            continue;
        }
        cursor += 1;
        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor >= attrs.len) return null;
        if (attrs[cursor] == '"' or attrs[cursor] == '\'') {
            const quote = attrs[cursor];
            const value_start = cursor + 1;
            const value_end = std.mem.indexOfScalarPos(u8, attrs, value_start, quote) orelse return null;
            return @as(?[]u8, try allocator.dupe(u8, attrs[value_start..value_end]));
        }
        const value_start = cursor;
        while (cursor < attrs.len and !htmlSpace(attrs[cursor]) and attrs[cursor] != '>') : (cursor += 1) {}
        return @as(?[]u8, try allocator.dupe(u8, attrs[value_start..cursor]));
    }
    return null;
}

fn explicitLabelForIdAlloc(allocator: std.mem.Allocator, html: []const u8, id: []const u8) !?[]u8 {
    var index: usize = 0;
    while (findOpeningTag(html, "label", index)) |start| {
        const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return null;
        const attrs = html[start + "<label".len .. open_end];
        const label_for = try htmlAttrValueAlloc(allocator, attrs, "for");
        defer if (label_for) |actual| allocator.free(actual);
        const close_start = std.mem.indexOfPos(u8, html, open_end + 1, "</label>") orelse return null;
        if (label_for) |actual_for| {
            if (std.mem.eql(u8, actual_for, id)) {
                const text = try htmlTextAlloc(allocator, html[open_end + 1 .. close_start]);
                const trimmed = std.mem.trim(u8, text, " \t\r\n");
                if (trimmed.len == text.len) return @as(?[]u8, text);
                const duped = try allocator.dupe(u8, trimmed);
                allocator.free(text);
                return @as(?[]u8, duped);
            }
        }
        index = close_start + "</label>".len;
    }
    return null;
}

fn wrappingLabelForControlAlloc(allocator: std.mem.Allocator, html: []const u8, tag_start: usize) !?[]u8 {
    const label_start = lastIndexOfBefore(html, "<label", tag_start) orelse return null;
    const previous_label_close = lastIndexOfBefore(html, "</label>", tag_start);
    if (previous_label_close != null and previous_label_close.? > label_start) return null;
    const label_open_end = std.mem.indexOfScalarPos(u8, html, label_start, '>') orelse return null;
    if (label_open_end >= tag_start) return null;
    const label_close = std.mem.indexOfPos(u8, html, tag_start, "</label>") orelse return null;
    if (label_close < tag_start) return null;
    const text = try htmlTextAlloc(allocator, html[label_open_end + 1 .. tag_start]);
    const trimmed = std.mem.trim(u8, text, " \t\r\n");
    if (trimmed.len == text.len) return @as(?[]u8, text);
    const duped = try allocator.dupe(u8, trimmed);
    allocator.free(text);
    return @as(?[]u8, duped);
}

fn findOpeningTag(html: []const u8, tag: []const u8, start_index: usize) ?usize {
    var index = start_index;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |start| {
        index = start + 1;
        const name_start = start + 1;
        const name_end = name_start + tag.len;
        if (name_end > html.len) continue;
        if (!std.ascii.eqlIgnoreCase(html[name_start..name_end], tag)) continue;
        if (name_end < html.len and htmlTagBoundary(html[name_end])) return start;
    }
    return null;
}

fn lastIndexOfBefore(haystack: []const u8, needle: []const u8, before: usize) ?usize {
    var result: ?usize = null;
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, haystack[0..@min(before, haystack.len)], index, needle)) |found| {
        result = found;
        index = found + 1;
    }
    return result;
}

fn htmlHasHeadingLevel(html: []const u8, level: u8) bool {
    var pattern: [3]u8 = .{ '<', 'h', level };
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, html, index, &pattern)) |start| {
        const after_level = start + pattern.len;
        if (after_level < html.len and htmlTagBoundary(html[after_level])) return true;
        index = after_level;
    }
    return false;
}

fn serverNowIsoAlloc(allocator: std.mem.Allocator) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    return sqliteNowIsoAlloc(allocator, db);
}

fn htmlDataTestIdCount(html: []const u8) usize {
    var count: usize = 0;
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, html, index, "data-testid")) |attr_start| {
        count += 1;
        index = attr_start + "data-testid".len;
    }
    return count;
}

fn countByte(value: []const u8, needle: u8) usize {
    var count: usize = 0;
    for (value) |char| {
        if (char == needle) count += 1;
    }
    return count;
}

fn htmlSpace(char: u8) bool {
    return char == ' ' or char == '\t' or char == '\n' or char == '\r';
}

fn htmlTagBoundary(char: u8) bool {
    return htmlSpace(char) or char == '>' or char == '/';
}

fn stringSliceLessThan(_: void, left: []const u8, right: []const u8) bool {
    return std.mem.lessThan(u8, left, right);
}

fn runtimeResourceUsageControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    return snapshotResourceUsageJsonAlloc(allocator, db, app_id);
}

fn openWebappControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const active = try activeSnapshotAppAlloc(allocator, db, app_id);
    const active_app = active orelse return error.AppNotInstalled;
    defer freeSnapshotActiveApp(allocator, active_app);

    const session_id = try randomDbIdAlloc(allocator, db, "session_");
    defer allocator.free(session_id);
    const started_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(started_at);
    const capabilities = try serverCapabilitiesJson(allocator);
    defer allocator.free(capabilities);
    const manifest_json = try activeManifestJsonInDbAlloc(allocator, db, app_id);
    defer allocator.free(manifest_json);
    try assertServerRequiredCapabilitiesAvailable(allocator, manifest_json);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) " ++
            "VALUES (?, 'zig-server', 'server', ?, ?, ?, ?, 'running', ?, '{\"storageBytes\":0,\"bridgeCalls\":0,\"coreEvents\":0}', '{\"source\":\"control\"}')",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, session_id);
    bindText(statement, 2, runtime_version);
    bindText(statement, 3, app_id);
    bindText(statement, 4, active_app.install_id);
    bindText(statement, 5, started_at);
    bindText(statement, 6, capabilities);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;

    const escaped_session_id = try escapeJsonString(allocator, session_id);
    defer allocator.free(escaped_session_id);
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, active_app.install_id);
    defer allocator.free(escaped_install_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"sessionId\":\"{s}\",\"appId\":\"{s}\",\"installId\":\"{s}\"}}",
        .{ escaped_session_id, escaped_app_id, escaped_install_id },
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

fn storageKeyHasAppPrefix(allocator: std.mem.Allocator, app_id: []const u8, key: []const u8) !bool {
    const prefix = try std.fmt.allocPrint(allocator, "{s}:", .{app_id});
    defer allocator.free(prefix);
    return std.mem.startsWith(u8, key, prefix);
}

fn appStoragePrefixDetailsJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, key: []const u8) ![]u8 {
    const prefix = try std.fmt.allocPrint(allocator, "{s}:", .{app_id});
    defer allocator.free(prefix);
    return storagePrefixDetailsJsonAlloc(allocator, key, prefix);
}

fn storagePrefixDetailsJsonAlloc(allocator: std.mem.Allocator, key: []const u8, prefix: []const u8) ![]u8 {
    const escaped_key = try escapeJsonString(allocator, key);
    defer allocator.free(escaped_key);
    const escaped_prefix = try escapeJsonString(allocator, prefix);
    defer allocator.free(escaped_prefix);
    return std.fmt.allocPrint(allocator, "{{\"key\":\"{s}\",\"prefix\":\"{s}\"}}", .{ escaped_key, escaped_prefix });
}

fn resetAppStorageControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const storage_rows = try deleteRowsForApp(db, "DELETE FROM app_storage WHERE app_id = ?", app_id);
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"appId\":\"{s}\",\"storageRowsDeleted\":{d}}}", .{ escaped_app_id, storage_rows });
}

fn resetWebappControl(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const storage_rows = try deleteRowsForApp(db, "DELETE FROM app_storage WHERE app_id = ?", app_id);
    const bridge_rows = try deleteRowsForApp(db, "DELETE FROM bridge_calls WHERE app_id = ?", app_id);
    const core_rows = try deleteRowsForApp(db, "DELETE FROM core_events WHERE app_id = ?", app_id);
    const test_rows = try deleteRowsForApp(db, "DELETE FROM test_runs WHERE app_id = ?", app_id);
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":true,\"appId\":\"{s}\",\"storageRowsDeleted\":{d},\"bridgeCallsDeleted\":{d},\"coreEventsDeleted\":{d},\"testRunsDeleted\":{d}}}",
        .{ escaped_app_id, storage_rows, bridge_rows, core_rows, test_rows },
    );
}

fn clearRuntimeLogsControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const bridge_rows = if (app_id) |actual_app_id|
        try deleteRowsForApp(db, "DELETE FROM bridge_calls WHERE app_id = ?", actual_app_id)
    else
        try deleteRows(db, "DELETE FROM bridge_calls");
    const core_rows = if (app_id) |actual_app_id|
        try deleteRowsForApp(db, "DELETE FROM core_events WHERE app_id = ?", actual_app_id)
    else
        try deleteRows(db, "DELETE FROM core_events");
    const test_rows = if (app_id) |actual_app_id|
        try deleteRowsForApp(db, "DELETE FROM test_runs WHERE app_id = ?", actual_app_id)
    else
        try deleteRows(db, "DELETE FROM test_runs");

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"ok\":true,\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.print(",\"bridgeCallsDeleted\":{d},\"coreEventsDeleted\":{d},\"testRunsDeleted\":{d}}}", .{ bridge_rows, core_rows, test_rows });
    return out.toOwnedSlice();
}

fn assertNoConsoleErrorsControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const errors = try countConsoleErrors(allocator, app_id);
    return std.fmt.allocPrint(allocator, "{{\"ok\":{},\"errors\":{d}}}", .{ errors == 0, errors });
}

fn countConsoleErrors(allocator: std.mem.Allocator, app_id: ?[]const u8) !i64 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const sql = if (app_id == null)
        "SELECT COUNT(*) FROM bridge_calls WHERE method = 'app.log' AND params_json LIKE '%\"level\":\"error\"%'"
    else
        "SELECT COUNT(*) FROM bridge_calls WHERE method = 'app.log' AND params_json LIKE '%\"level\":\"error\"%' AND app_id = ?";
    if (app_id) |actual_app_id| return int64QueryDb(db, sql, actual_app_id);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn callBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params_opt: ?std.json.Value,
) ![]u8 {
    var empty_params = try std.json.parseFromSlice(std.json.Value, allocator, "{}", .{});
    defer empty_params.deinit();
    const params = params_opt orelse empty_params.value;
    const params_json = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json);
    if (params.object.get("appId") != null) {
        return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "invalid_request", "Bridge params must not include appId; app id is channel-derived", "{\"field\":\"appId\"}");
    }

    if (!isAllowedRuntimeBridgeMethod(method)) {
        if (isKnownUnsupportedBridgeMethod(method)) {
            const details_json = try methodDetailsJsonAlloc(allocator, method);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "platform_unsupported", "Bridge method is not implemented on zig-server", details_json);
        }
        const details_json = try methodDetailsJsonAlloc(allocator, method);
        defer allocator.free(details_json);
        return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "unknown_method", "Unknown bridge method", details_json);
    }

    const compatible_runtime = bridgeRuntimeCompatible(allocator, app_id) catch {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "runtime_compatibility_unavailable", "Runtime compatibility could not be evaluated");
    };
    if (!compatible_runtime) {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "runtime_version_incompatible", "App runtimeVersion is not compatible with the zig-server runtime");
    }

    if (try takeInjectedFaultAlloc(allocator, app_id, session_id, method)) |fault| {
        defer freeFaultInjection(allocator, fault);
        const error_json = try faultBridgeErrorJsonAlloc(allocator, fault, app_id, method);
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeErrorResponseJsonAlloc(allocator, id, fault.code, fault.message);
    }

    if (permissionForBridgeMethod(method)) |permission| {
        const permitted = bridgePermissionApproved(allocator, app_id, permission) catch false;
        if (!permitted) {
            const details_json = try permissionDetailsJsonAlloc(allocator, app_id, method, permission);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "permission_denied", "Bridge method requires an approved app permission", details_json);
        }
    }

    const budget_violation = enforceBridgeResourceBudget(allocator, app_id, method, params) catch {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "resource_budget_unavailable", "Resource budget could not be evaluated");
    };
    if (budget_violation) |violation| {
        const details_json = try resourceBudgetDetailsJsonAlloc(allocator, violation);
        defer allocator.free(details_json);
        return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "resource_budget_exceeded", violation.message, details_json);
    }

    if (std.mem.eql(u8, method, "core.step")) {
        if (valueString(params.object.get("app"))) |requested_app| {
            if (!std.mem.eql(u8, requested_app, app_id)) {
                return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "permission_denied", "core.step app field does not match the channel-derived app id");
            }
        }
        const result_json = coreStepAlloc(allocator, params_json) catch {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "core_error", "core.step failed");
        };
        defer allocator.free(result_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        const event_json = if (params.object.get("event")) |event_value|
            try jsonValueAlloc(allocator, event_value)
        else
            try allocator.dupe(u8, "{}");
        defer allocator.free(event_json);
        recordCoreStep(allocator, app_id, session_id orelse "server-dev-session", event_json, result_json) catch |err| {
            std.debug.print("core audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, result_json);
    }

    if (std.mem.eql(u8, method, "runtime.capabilities")) {
        const result_json = try serverCapabilitiesForAppJson(allocator, app_id);
        defer allocator.free(result_json);
        logBridgeCall(allocator, app_id, session_id, method, "{}", result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, result_json);
    }

    if (std.mem.startsWith(u8, method, "storage.")) {
        return storageBridgeControl(allocator, app_id, session_id, id, method, params, params_json);
    }
    if (std.mem.eql(u8, method, "app.log")) {
        return appLogBridgeControl(allocator, app_id, session_id, id, method, params, params_json);
    }
    if (std.mem.eql(u8, method, "notification.toast")) {
        return notificationToastBridgeControl(allocator, app_id, session_id, id, method, params, params_json);
    }
    if (std.mem.eql(u8, method, "network.request")) {
        return networkRequestBridgeControl(allocator, app_id, session_id, id, method, params, params_json);
    }
    if (std.mem.eql(u8, method, "dialog.openFile") or std.mem.eql(u8, method, "dialog.saveFile")) {
        return dialogBridgeControl(allocator, app_id, session_id, id, method, params_json);
    }

    return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "unknown_method", "Unknown bridge method");
}

fn coreStepControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    event: std.json.Value,
) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"event\":");
    try std.json.Stringify.value(event, .{}, &out.writer);
    try out.writer.writeAll("}");
    const params_json = try out.toOwnedSlice();
    defer allocator.free(params_json);

    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, params_json, .{});
    defer parsed.deinit();
    return callBridgeControl(allocator, app_id, session_id, id, "core.step", parsed.value);
}

fn storageBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    params_json: []const u8,
) ![]u8 {
    if (std.mem.eql(u8, method, "storage.get")) {
        const key = valueString(params.object.get("key")) orelse {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "storage.get requires key");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, key))) {
            const details_json = try appStoragePrefixDetailsJsonAlloc(allocator, app_id, key);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "permission_denied", "Storage key must begin with app storage prefix", details_json);
        }
        const result_json = storageGetResultJson(allocator, app_id, key, params.object.get("defaultValue")) catch {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "storage_error", "storage.get failed");
        };
        defer allocator.free(result_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, result_json);
    }

    if (std.mem.eql(u8, method, "storage.set")) {
        const key = valueString(params.object.get("key")) orelse {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "storage.set requires key");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, key))) {
            const details_json = try appStoragePrefixDetailsJsonAlloc(allocator, app_id, key);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "permission_denied", "Storage key must begin with app storage prefix", details_json);
        }
        const value = if (params.object.get("value")) |value_param|
            try jsonValueAlloc(allocator, value_param)
        else
            try allocator.dupe(u8, "null");
        defer allocator.free(value);
        storageSet(app_id, key, value) catch {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "storage_error", "storage.set failed");
        };
        const result_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"bytesWritten\":{d}}}", .{value.len});
        defer allocator.free(result_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, result_json);
    }

    if (std.mem.eql(u8, method, "storage.remove")) {
        const key = valueString(params.object.get("key")) orelse {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "storage.remove requires key");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, key))) {
            const details_json = try appStoragePrefixDetailsJsonAlloc(allocator, app_id, key);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "permission_denied", "Storage key must begin with app storage prefix", details_json);
        }
        storageRemove(app_id, key) catch {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "storage_error", "storage.remove failed");
        };
        logBridgeCall(allocator, app_id, session_id, method, params_json, "{\"ok\":true}", null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, "{\"ok\":true}");
    }

    if (std.mem.eql(u8, method, "storage.list")) {
        const prefix = valueString(params.object.get("prefix")) orelse {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "storage.list requires prefix");
        };
        if (!(try storageKeyHasAppPrefix(allocator, app_id, prefix))) {
            const details_json = try appStoragePrefixDetailsJsonAlloc(allocator, app_id, prefix);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "permission_denied", "Storage key must begin with app storage prefix", details_json);
        }
        const result_json = storageListResultJson(allocator, app_id, prefix) catch {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "storage_error", "storage.list failed");
        };
        defer allocator.free(result_json);
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return bridgeOkJsonAlloc(allocator, id, result_json);
    }

    return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "unknown_method", "Unknown storage method");
}

fn appLogBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    params_json: []const u8,
) ![]u8 {
    const level = valueString(params.object.get("level")) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "app.log requires level");
    };
    if (!isLogLevel(level)) {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "app.log level must be debug, info, warn, or error");
    }
    const message = valueString(params.object.get("message")) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "app.log requires message");
    };
    logAppMessage(allocator, app_id, session_id, level, message) catch {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "storage_error", "app.log failed");
    };
    return bridgeOkJsonAlloc(allocator, id, "{\"ok\":true}");
}

fn notificationToastBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    params_json: []const u8,
) ![]u8 {
    _ = valueString(params.object.get("message")) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "notification.toast requires message");
    };
    if (params.object.get("level")) |level_value| {
        const level = valueString(level_value) orelse {
            return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "notification.toast level must be a string");
        };
        if (!isToastLevel(level)) {
            const details_json = try toastLevelDetailsJsonAlloc(allocator, level);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "invalid_request", "notification.toast level must be info, success, warning, or error", details_json);
        }
    }
    logBridgeCall(allocator, app_id, session_id, method, params_json, "{\"ok\":true}", null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return bridgeOkJsonAlloc(allocator, id, "{\"ok\":true}");
}

fn networkRequestBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    params_json: []const u8,
) ![]u8 {
    const url = valueString(params.object.get("url")) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "invalid_request", "network.request requires url");
    };
    const request_method_raw = valueString(params.object.get("method")) orelse "GET";
    const request_method = try upperAsciiAlloc(allocator, request_method_raw);
    defer allocator.free(request_method);
    const deny_details = networkRequestDenyDetailsJsonAlloc(allocator, app_id, url, request_method, params) catch |err| switch (err) {
        error.InvalidNetworkUrl => {
            const details_json = try networkUrlDetailsJsonAlloc(allocator, url);
            defer allocator.free(details_json);
            return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "invalid_request", "network.request url must be absolute", details_json);
        },
        else => return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "network_policy_denied", "network.request is outside manifest.networkPolicy"),
    };
    if (deny_details) |details_json| {
        defer allocator.free(details_json);
        return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, "network_policy_denied", "network.request is outside manifest.networkPolicy", details_json);
    }
    const result_json = (try networkMockResultJsonAlloc(allocator, app_id, session_id, request_method, url)) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "network.mock_missing", "No network mock is registered for request");
    };
    defer allocator.free(result_json);
    if (try networkResponsePolicyErrorAlloc(allocator, app_id, url, request_method, params, result_json)) |policy_error| {
        defer freeNetworkPolicyBridgeError(allocator, policy_error);
        return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, policy_error.code, policy_error.message, policy_error.details_json);
    }
    const response_payload_json = try networkResponsePayloadJsonAlloc(allocator, result_json);
    defer allocator.free(response_payload_json);
    logBridgeCall(allocator, app_id, session_id, method, params_json, response_payload_json, null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return bridgeOkJsonAlloc(allocator, id, response_payload_json);
}

fn dialogBridgeControl(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params_json: []const u8,
) ![]u8 {
    const dialog_type = dialogTypeForBridgeMethod(method) orelse {
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "unknown_method", "Unknown dialog method");
    };
    const result_json = (try dialogMockResultJsonAlloc(allocator, app_id, session_id, dialog_type)) orelse {
        if (std.mem.eql(u8, dialog_type, "saveFile")) {
            logBridgeCall(allocator, app_id, session_id, method, params_json, "{\"ok\":true}", null) catch |err| {
                std.debug.print("bridge audit write failed: {}\n", .{err});
            };
            return bridgeOkJsonAlloc(allocator, id, "{\"ok\":true}");
        }
        return bridgeControlErrorResponse(allocator, app_id, session_id, id, method, params_json, "dialog.mock_missing", "No dialog.openFile mock is registered");
    };
    defer allocator.free(result_json);
    logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return bridgeOkJsonAlloc(allocator, id, result_json);
}

fn assertStorageControl(allocator: std.mem.Allocator, app_id: []const u8, key: []const u8, expected: std.json.Value) ![]u8 {
    const actual_json = try appStorageValueJsonAlloc(allocator, app_id, key);
    defer allocator.free(actual_json);
    var parsed_actual = std.json.parseFromSlice(std.json.Value, allocator, if (actual_json.len == 0) "null" else actual_json, .{}) catch return error.AssertionFailed;
    defer parsed_actual.deinit();
    if (!jsonValuesEqual(parsed_actual.value, expected)) return error.AssertionFailed;

    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_key = try escapeJsonString(allocator, key);
    defer allocator.free(escaped_key);
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"appId\":\"{s}\",\"key\":\"{s}\",\"value\":{s}}}", .{ escaped_app_id, escaped_key, actual_json });
}

fn assertBridgeCallControl(allocator: std.mem.Allocator, app_id: []const u8, method: []const u8) ![]u8 {
    const count = try countBridgeCallsByMethod(allocator, app_id, method);
    if (count == 0) return error.AssertionFailed;
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"appId\":\"{s}\",\"method\":\"{s}\",\"count\":{d}}}", .{ escaped_app_id, escaped_method, count });
}

fn assertCoreActionControl(allocator: std.mem.Allocator, app_id: []const u8, expected_type: ?[]const u8, expected_match: ?std.json.Value) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT action_json FROM core_actions WHERE app_id = ? ORDER BY created_at", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);

    var actions: std.io.Writer.Allocating = .init(allocator);
    errdefer actions.deinit();
    try actions.writer.writeAll("[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const action_json = sqliteColumnText(statement, 0);
        var parsed = std.json.parseFromSlice(std.json.Value, allocator, action_json, .{}) catch continue;
        defer parsed.deinit();
        if (parsed.value != .object) continue;
        if (expected_type) |actual_expected_type| {
            const action_type = valueString(parsed.value.object.get("type")) orelse continue;
            if (!std.mem.eql(u8, action_type, actual_expected_type)) continue;
        }
        if (!jsonMatchesSubset(parsed.value, expected_match)) continue;
        if (count > 0) try actions.writer.writeAll(",");
        try actions.writer.writeAll(action_json);
        count += 1;
    }
    try actions.writer.writeAll("]");
    if (count == 0) return error.AssertionFailed;
    const actions_json = try actions.toOwnedSlice();
    defer allocator.free(actions_json);

    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print("{{\"ok\":true,\"appId\":\"{s}\",\"type\":", .{escaped_app_id});
    try appendJsonNullableString(allocator, &out, expected_type);
    try out.writer.print(",\"count\":{d},\"actions\":{s}}}", .{ count, actions_json });
    return out.toOwnedSlice();
}

fn coreSnapshotControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    if (app_id) |actual_app_id| {
        const state_version = try coreStateVersionForApp(db, actual_app_id);
        const escaped_app_id = try escapeJsonString(allocator, actual_app_id);
        defer allocator.free(escaped_app_id);
        return std.fmt.allocPrint(allocator, "{{\"appId\":\"{s}\",\"stateVersion\":{d}}}", .{ escaped_app_id, state_version });
    }

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT app_id, COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) AS state_version FROM core_events WHERE app_id IS NOT NULL GROUP BY app_id ORDER BY app_id",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"apps\":[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{\"appId\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 0));
        try out.writer.print(",\"stateVersion\":{d}}}", .{sqlite.sqlite3_column_int64(statement, 1)});
        count += 1;
    }
    try out.writer.writeAll("]}");
    return out.toOwnedSlice();
}

fn replayEventsControl(allocator: std.mem.Allocator, app_id: []const u8, events: std.json.Value) ![]u8 {
    const core = core_api.core_create() orelse {
        return error.CoreCreateFailed;
    };
    defer core_api.core_destroy(core);

    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    var replay: std.io.Writer.Allocating = .init(allocator);
    errdefer replay.deinit();
    try replay.writer.print("{{\"ok\":true,\"appId\":\"{s}\",\"replay\":[", .{escaped_app_id});
    for (events.array.items, 0..) |event, index| {
        var body: std.io.Writer.Allocating = .init(allocator);
        errdefer body.deinit();
        try body.writer.writeAll("{\"event\":");
        try std.json.Stringify.value(event, .{}, &body.writer);
        try body.writer.writeAll("}");
        const body_json = try body.toOwnedSlice();
        defer allocator.free(body_json);
        const result_json = try coreStepWithCoreAlloc(allocator, core, body_json);
        defer allocator.free(result_json);
        const event_json = try jsonValueAlloc(allocator, event);
        defer allocator.free(event_json);
        if (index > 0) try replay.writer.writeAll(",");
        try replay.writer.print("{{\"event\":{s},\"result\":{s}}}", .{ event_json, result_json });
    }
    try replay.writer.writeAll("]}");
    return replay.toOwnedSlice();
}

fn appStorageValueJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, key: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.AssertionFailed;
    return allocator.dupe(u8, sqliteColumnNullableText(statement, 0) orelse "null");
}

fn countBridgeCallsByMethod(allocator: std.mem.Allocator, app_id: []const u8, method: []const u8) !i64 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, method);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn coreStateVersionForApp(db: *sqlite.sqlite3, app_id: []const u8) !i64 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn jsonValuesEqual(actual: std.json.Value, expected: std.json.Value) bool {
    if (actual == .null or expected == .null) return actual == .null and expected == .null;
    if (actual == .bool or expected == .bool) return actual == .bool and expected == .bool and actual.bool == expected.bool;
    if (actual == .integer or expected == .integer) return actual == .integer and expected == .integer and actual.integer == expected.integer;
    if (actual == .float or expected == .float) return actual == .float and expected == .float and actual.float == expected.float;
    if (actual == .number_string or expected == .number_string) return actual == .number_string and expected == .number_string and std.mem.eql(u8, actual.number_string, expected.number_string);
    if (actual == .string or expected == .string) return actual == .string and expected == .string and std.mem.eql(u8, actual.string, expected.string);
    if (actual == .array or expected == .array) {
        if (actual != .array or expected != .array) return false;
        if (actual.array.items.len != expected.array.items.len) return false;
        for (actual.array.items, expected.array.items) |actual_item, expected_item| {
            if (!jsonValuesEqual(actual_item, expected_item)) return false;
        }
        return true;
    }
    if (actual == .object or expected == .object) {
        if (actual != .object or expected != .object) return false;
        if (actual.object.count() != expected.object.count()) return false;
        var iterator = expected.object.iterator();
        while (iterator.next()) |entry| {
            const actual_value = actual.object.get(entry.key_ptr.*) orelse return false;
            if (!jsonValuesEqual(actual_value, entry.value_ptr.*)) return false;
        }
        return true;
    }
    return false;
}

fn jsonMatchesSubset(actual: std.json.Value, expected_opt: ?std.json.Value) bool {
    const expected = expected_opt orelse return true;
    if (expected == .null) return true;
    if (expected == .object) {
        if (actual != .object) return false;
        var iterator = expected.object.iterator();
        while (iterator.next()) |entry| {
            const actual_value = actual.object.get(entry.key_ptr.*) orelse return false;
            if (!jsonMatchesSubset(actual_value, entry.value_ptr.*)) return false;
        }
        return true;
    }
    return jsonValuesEqual(actual, expected);
}

fn deleteRowsForApp(db: *sqlite.sqlite3, sql: [*:0]const u8, app_id: []const u8) !i64 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    return sqlite.sqlite3_changes(db);
}

fn deleteRows(db: *sqlite.sqlite3, sql: [*:0]const u8) !i64 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    return sqlite.sqlite3_changes(db);
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
    return evaluateSmokeTestSourcesAlloc(
        allocator,
        app_id,
        findPackageFile(files, "smoke-tests.json"),
        findPackageFile(files, "index.html") orelse "",
        findPackageFile(files, "app.js") orelse "",
    );
}

fn evaluateInstalledSmokeTestsAlloc(allocator: std.mem.Allocator, app_id: []const u8) !SmokeTestEvaluation {
    const package = try runtimeHtmlPackageAlloc(allocator, app_id);
    defer freeRuntimeHtmlPackage(allocator, package);
    const app_js = try installedAppJsAlloc(allocator, app_id);
    defer allocator.free(app_js);
    const smoke_tests = try installedPackageFileAlloc(allocator, app_id, "smoke-tests.json");
    defer if (smoke_tests) |actual| allocator.free(actual);
    return evaluateSmokeTestSourcesAlloc(allocator, app_id, smoke_tests, package.html, app_js);
}

fn evaluateSmokeTestSourcesAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    smoke_tests_maybe: ?[]const u8,
    html: []const u8,
    app_js: []const u8,
) !SmokeTestEvaluation {
    const smoke_tests = smoke_tests_maybe orelse {
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

fn activeInstallIdForAppAlloc(allocator: std.mem.Allocator, app_id: []const u8) !?[]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    return activeInstallIdAlloc(allocator, db, app_id);
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

    return coreStepWithCoreAlloc(allocator, core, body);
}

fn coreStepWithCoreAlloc(allocator: std.mem.Allocator, core: *core_api.Core, body: []const u8) ![]u8 {
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
        \\  resource_high_water_json TEXT,
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
        \\CREATE TABLE IF NOT EXISTS fault_injections (
        \\  fault_id TEXT PRIMARY KEY,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
        \\  app_id TEXT,
        \\  method TEXT NOT NULL,
        \\  code TEXT NOT NULL,
        \\  message TEXT NOT NULL,
        \\  details_json TEXT,
        \\  once INTEGER NOT NULL DEFAULT 1,
        \\  enabled INTEGER NOT NULL DEFAULT 1,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_test_runs_session_started ON test_runs(session_id, started_at);
        \\CREATE INDEX IF NOT EXISTS idx_test_runs_app_started ON test_runs(app_id, started_at);
        \\CREATE INDEX IF NOT EXISTS idx_network_mocks_session_app ON network_mocks(session_id, app_id);
        \\CREATE INDEX IF NOT EXISTS idx_dialog_mocks_session_app ON dialog_mocks(session_id, app_id);
        \\CREATE INDEX IF NOT EXISTS idx_fault_injections_method_app ON fault_injections(method, app_id, enabled);
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
    _ = sqlite.sqlite3_exec(db, "ALTER TABLE runtime_sessions ADD COLUMN resource_high_water_json TEXT;", null, null, null);
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
    const fault_injections = try queryRowsJson(allocator, "SELECT fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at FROM fault_injections ORDER BY created_at", null);
    defer allocator.free(fault_injections);

    return std.fmt.allocPrint(
        allocator,
        "{{\"apps\":{s},\"app_versions\":{s},\"app_installations\":{s},\"app_install_reports\":{s},\"app_storage\":{s},\"bridge_calls\":{s},\"control_sessions\":{s},\"control_commands\":{s},\"runtime_sessions\":{s},\"runtime_snapshots\":{s},\"app_migrations\":{s},\"migration_runs\":{s},\"core_events\":{s},\"test_runs\":{s},\"fault_injections\":{s}}}",
        .{ apps, app_versions, app_installations, app_install_reports, storage, bridge_calls, control_sessions, control_commands, runtime_sessions, runtime_snapshots, app_migrations, migration_runs, core_events, test_runs, fault_injections },
    );
}

fn dbBackupExportJson(allocator: std.mem.Allocator) ![]u8 {
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

    const base_json = try std.fmt.allocPrint(
        allocator,
        "{{\"exportId\":\"{s}\",\"type\":\"backup\",\"createdAt\":\"{s}\",\"runtimeVersion\":\"{s}\",\"source\":{{\"platform\":\"server\",\"target\":\"zig-server\"}},\"apps\":{s},\"appVersions\":{s},\"appFiles\":{s},\"appPermissions\":{s},\"appStorage\":{s},\"appMigrations\":{s},\"appInstallReports\":{s},\"runtimeCapabilities\":{s},\"debug\":{{}}}}",
        .{ export_id, created_at, runtime_version, apps, app_versions, app_files, app_permissions, storage, app_migrations, install_reports, capabilities },
    );
    defer allocator.free(base_json);
    const content_hash = try sha256HexAlloc(allocator, base_json);
    defer allocator.free(content_hash);
    return std.fmt.allocPrint(allocator, "{s},\"contentHash\":\"sha256:{s}\"}}", .{ base_json[0 .. base_json.len - 1], content_hash });
}

fn importBackupControl(allocator: std.mem.Allocator, backup: std.json.Value) ![]u8 {
    if (backup != .object) return error.InvalidBackup;
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
    const backup_json = try jsonValueAlloc(allocator, backup);
    defer allocator.free(backup_json);
    const content_hash = if (objectString(backup, "contentHash")) |hash|
        try allocator.dupe(u8, hash)
    else
        try sha256PrefixedAlloc(allocator, backup_json);
    defer allocator.free(content_hash);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const apps_count = try importBackupApps(backup.object.get("apps"), db, created_at);
    const versions_count = try importBackupAppVersions(allocator, backup.object.get("appVersions"), db, created_at);
    _ = try importBackupAppFiles(allocator, backup.object.get("appFiles"), db, created_at);
    _ = try importBackupAppPermissions(backup.object.get("appPermissions"), db);
    const storage_count = try importBackupAppStorage(allocator, backup.object.get("appStorage"), db, created_at);
    _ = try importBackupAppMigrations(allocator, backup.object.get("appMigrations"), db, created_at);
    _ = try importBackupInstallReports(allocator, backup.object.get("appInstallReports"), db, created_at);
    try insertBackupImportRecord(db, allocator, backup, backup_json, content_hash, created_at);

    const result_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"apps\":{d},\"appVersions\":{d},\"appStorage\":{d}}}", .{ apps_count, versions_count, storage_count });
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn importBackupApps(value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const apps = value orelse return 0;
    if (apps != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (apps.array.items) |app| {
        if (app != .object) return error.InvalidBackup;
        const app_id = objectStringAny(app, "id", "appId") orelse return error.InvalidBackup;
        const name = objectString(app, "name") orelse app_id;
        const status = objectString(app, "status") orelse "enabled";
        const data_version = objectI64Any(app, "data_version", "dataVersion") orelse 1;
        const row_created_at = objectStringAny(app, "created_at", "createdAt") orelse created_at;
        const row_updated_at = objectStringAny(app, "updated_at", "updatedAt") orelse created_at;
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, app_id);
        bindText(statement, 2, name);
        bindText(statement, 3, status);
        bindNullableText(statement, 4, objectStringAny(app, "active_install_id", "activeInstallId"));
        bindNullableText(statement, 5, objectStringAny(app, "active_version", "activeVersion"));
        _ = sqlite.sqlite3_bind_int64(statement, 6, data_version);
        bindText(statement, 7, row_created_at);
        bindText(statement, 8, row_updated_at);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupAppVersions(allocator: std.mem.Allocator, value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const versions = value orelse return 0;
    if (versions != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (versions.array.items) |version| {
        if (version != .object) return error.InvalidBackup;
        const install_id = objectStringAny(version, "install_id", "installId") orelse return error.InvalidBackup;
        const app_id = objectStringAny(version, "app_id", "appId") orelse return error.InvalidBackup;
        const version_name = objectString(version, "version") orelse objectString(version, "appVersion") orelse "0.0.0";
        const version_runtime = objectStringAny(version, "runtime_version", "runtimeVersion") orelse runtime_version;
        const data_version = objectI64Any(version, "data_version", "dataVersion") orelse 1;
        const manifest_json = try jsonDocumentFieldAlloc(allocator, version, "manifest_json", "manifestJson", "manifest", "{}");
        defer allocator.free(manifest_json);
        const signature_json = try optionalJsonDocumentFieldAlloc(allocator, version, "signature_json", "signatureJson", "signature");
        defer if (signature_json) |json| allocator.free(json);
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, install_id);
        bindText(statement, 2, app_id);
        bindText(statement, 3, version_name);
        bindText(statement, 4, version_runtime);
        _ = sqlite.sqlite3_bind_int64(statement, 5, data_version);
        bindText(statement, 6, manifest_json);
        bindText(statement, 7, objectStringAny(version, "manifest_hash", "manifestHash") orelse "");
        bindText(statement, 8, objectStringAny(version, "content_hash", "contentHash") orelse "");
        bindNullableText(statement, 9, signature_json);
        bindText(statement, 10, objectStringAny(version, "trust_level", "trustLevel") orelse "developer");
        bindText(statement, 11, objectString(version, "status") orelse "installed");
        bindText(statement, 12, objectStringAny(version, "created_at", "installedAt") orelse objectStringAny(version, "createdAt", "created_at") orelse created_at);
        bindNullableText(statement, 13, objectStringAny(version, "activated_at", "activatedAt"));
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupAppFiles(allocator: std.mem.Allocator, value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const files = value orelse return 0;
    if (files != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (files.array.items) |file| {
        if (file != .object) return error.InvalidBackup;
        const install_id = objectStringAny(file, "install_id", "installId") orelse return error.InvalidBackup;
        const path = objectString(file, "path") orelse return error.InvalidBackup;
        const content = objectStringAny(file, "content_text", "contentText") orelse "";
        const generated_hash = try sha256PrefixedAlloc(allocator, content);
        defer allocator.free(generated_hash);
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, install_id);
        bindText(statement, 2, path);
        bindText(statement, 3, content);
        bindText(statement, 4, objectStringAny(file, "content_hash", "contentHash") orelse generated_hash);
        _ = sqlite.sqlite3_bind_int64(statement, 5, objectI64Any(file, "size_bytes", "sizeBytes") orelse @as(i64, @intCast(content.len)));
        bindText(statement, 6, objectString(file, "mime") orelse mimeForPackagePath(path));
        bindText(statement, 7, objectStringAny(file, "created_at", "createdAt") orelse created_at);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupAppPermissions(value: ?std.json.Value, db: *sqlite.sqlite3) !usize {
    const permissions = value orelse return 0;
    if (permissions != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (permissions.array.items) |permission| {
        if (permission != .object) return error.InvalidBackup;
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, objectStringAny(permission, "install_id", "installId") orelse return error.InvalidBackup);
        bindText(statement, 2, objectStringAny(permission, "app_id", "appId") orelse "");
        bindText(statement, 3, objectString(permission, "permission") orelse return error.InvalidBackup);
        _ = sqlite.sqlite3_bind_int64(statement, 4, objectI64(permission, "requested") orelse 1);
        _ = sqlite.sqlite3_bind_int64(statement, 5, objectBoolInt(permission, "approved") orelse 0);
        bindNullableText(statement, 6, objectStringAny(permission, "approved_at", "approvedAt"));
        bindNullableText(statement, 7, objectString(permission, "reason") orelse "imported");
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupAppStorage(allocator: std.mem.Allocator, value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const storage_rows = value orelse return 0;
    if (storage_rows != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (storage_rows.array.items) |storage| {
        if (storage != .object) return error.InvalidBackup;
        const value_json = try jsonDocumentFieldAlloc(allocator, storage, "value_json", "valueJson", "value", "null");
        defer allocator.free(value_json);
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, objectStringAny(storage, "app_id", "appId") orelse return error.InvalidBackup);
        bindText(statement, 2, objectString(storage, "key") orelse return error.InvalidBackup);
        bindText(statement, 3, value_json);
        bindText(statement, 4, objectStringAny(storage, "updated_at", "updatedAt") orelse created_at);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupAppMigrations(allocator: std.mem.Allocator, value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const migrations = value orelse return 0;
    if (migrations != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (migrations.array.items) |migration| {
        if (migration != .object) return error.InvalidBackup;
        const fallback_id = try randomDbIdAlloc(allocator, db, "migration_");
        defer allocator.free(fallback_id);
        const migration_json = try jsonDocumentFieldAlloc(allocator, migration, "migration_json", "migrationJson", "migration", "{}");
        defer allocator.free(migration_json);
        const generated_hash = try sha256PrefixedAlloc(allocator, migration_json);
        defer allocator.free(generated_hash);
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, objectStringAny(migration, "migration_id", "migrationId") orelse fallback_id);
        bindText(statement, 2, objectStringAny(migration, "app_id", "appId") orelse return error.InvalidBackup);
        _ = sqlite.sqlite3_bind_int64(statement, 3, objectI64Any(migration, "from_data_version", "fromDataVersion") orelse return error.InvalidBackup);
        _ = sqlite.sqlite3_bind_int64(statement, 4, objectI64Any(migration, "to_data_version", "toDataVersion") orelse return error.InvalidBackup);
        bindText(statement, 5, migration_json);
        bindText(statement, 6, objectStringAny(migration, "content_hash", "contentHash") orelse generated_hash);
        bindText(statement, 7, objectStringAny(migration, "created_at", "createdAt") orelse created_at);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn importBackupInstallReports(allocator: std.mem.Allocator, value: ?std.json.Value, db: *sqlite.sqlite3, created_at: []const u8) !usize {
    const reports = value orelse return 0;
    if (reports != .array) return error.InvalidBackup;
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    var count: usize = 0;
    for (reports.array.items) |report| {
        if (report != .object) return error.InvalidBackup;
        const fallback_id = try randomDbIdAlloc(allocator, db, "report_");
        defer allocator.free(fallback_id);
        const validation_json = try optionalJsonDocumentFieldAlloc(allocator, report, "validation_json", "validationJson", "validation");
        defer if (validation_json) |json| allocator.free(json);
        const security_json = try optionalJsonDocumentFieldAlloc(allocator, report, "security_json", "securityJson", "security");
        defer if (security_json) |json| allocator.free(json);
        const permissions_json = try optionalJsonDocumentFieldAlloc(allocator, report, "permissions_json", "permissionsJson", "permissions");
        defer if (permissions_json) |json| allocator.free(json);
        const compatibility_json = try optionalJsonDocumentFieldAlloc(allocator, report, "compatibility_json", "compatibilityJson", "compatibility");
        defer if (compatibility_json) |json| allocator.free(json);
        const smoke_test_json = try optionalJsonDocumentFieldAlloc(allocator, report, "smoke_test_json", "smokeTestJson", "smokeTest");
        defer if (smoke_test_json) |json| allocator.free(json);
        _ = sqlite.sqlite3_reset(statement);
        _ = sqlite.sqlite3_clear_bindings(statement);
        bindText(statement, 1, objectStringAny(report, "report_id", "reportId") orelse fallback_id);
        bindText(statement, 2, objectStringAny(report, "app_id", "appId") orelse return error.InvalidBackup);
        bindNullableText(statement, 3, objectStringAny(report, "install_id", "installId"));
        bindText(statement, 4, objectString(report, "status") orelse "accepted");
        bindNullableText(statement, 5, validation_json);
        bindNullableText(statement, 6, security_json);
        bindNullableText(statement, 7, permissions_json);
        bindNullableText(statement, 8, compatibility_json);
        bindNullableText(statement, 9, smoke_test_json);
        bindNullableText(statement, 10, objectStringAny(report, "content_hash", "contentHash"));
        bindText(statement, 11, objectStringAny(report, "created_at", "createdAt") orelse created_at);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
        count += 1;
    }
    return count;
}

fn insertBackupImportRecord(
    db: *sqlite.sqlite3,
    allocator: std.mem.Allocator,
    backup: std.json.Value,
    backup_json: []const u8,
    content_hash: []const u8,
    created_at: []const u8,
) !void {
    const import_id = try randomDbIdAlloc(allocator, db, "import_");
    defer allocator.free(import_id);
    const runtime = objectString(backup, "runtimeVersion") orelse runtime_version;
    const source_platform = if (backup.object.get("source")) |source|
        if (source == .object) objectString(source, "platform") orelse "unknown" else "unknown"
    else
        "unknown";
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at) VALUES (?, 'import', ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, import_id);
    bindText(statement, 2, source_platform);
    bindText(statement, 3, runtime);
    bindText(statement, 4, backup_json);
    bindText(statement, 5, content_hash);
    bindText(statement, 6, created_at);
    bindText(statement, 7, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn objectString(value: std.json.Value, field: []const u8) ?[]const u8 {
    if (value != .object) return null;
    return valueString(value.object.get(field));
}

fn objectStringAny(value: std.json.Value, first: []const u8, second: []const u8) ?[]const u8 {
    return objectString(value, first) orelse objectString(value, second);
}

fn objectI64(value: std.json.Value, field: []const u8) ?i64 {
    if (value != .object) return null;
    return valueI64(value.object.get(field));
}

fn objectI64Any(value: std.json.Value, first: []const u8, second: []const u8) ?i64 {
    return objectI64(value, first) orelse objectI64(value, second);
}

fn objectBoolInt(value: std.json.Value, field: []const u8) ?i64 {
    if (value != .object) return null;
    const actual = value.object.get(field) orelse return null;
    if (actual == .bool) return if (actual.bool) 1 else 0;
    return valueI64(actual);
}

fn optionalJsonDocumentFieldAlloc(
    allocator: std.mem.Allocator,
    object: std.json.Value,
    raw_field: []const u8,
    camel_field: []const u8,
    value_field: []const u8,
) !?[]u8 {
    if (objectStringAny(object, raw_field, camel_field)) |json| return try allocator.dupe(u8, json);
    if (object == .object) {
        if (object.object.get(value_field)) |value| return try jsonValueAlloc(allocator, value);
    }
    return null;
}

fn jsonDocumentFieldAlloc(
    allocator: std.mem.Allocator,
    object: std.json.Value,
    raw_field: []const u8,
    camel_field: []const u8,
    value_field: []const u8,
    default_json: []const u8,
) ![]u8 {
    return (try optionalJsonDocumentFieldAlloc(allocator, object, raw_field, camel_field, value_field)) orelse try allocator.dupe(u8, default_json);
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
    const fault_injections = try queryRowsJson(allocator, "SELECT fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at FROM fault_injections ORDER BY created_at", null);
    defer allocator.free(fault_injections);

    const base_json = try std.fmt.allocPrint(
        allocator,
        "{{\"exportId\":\"{s}\",\"type\":\"debug-bundle\",\"createdAt\":\"{s}\",\"runtimeVersion\":\"{s}\",\"source\":{{\"platform\":\"server\",\"target\":\"zig-server\"}},\"apps\":{s},\"appVersions\":{s},\"appFiles\":{s},\"appPermissions\":{s},\"appStorage\":{s},\"appMigrations\":{s},\"appInstallReports\":{s},\"runtimeCapabilities\":{s},\"debug\":{{\"runtimeSessions\":{s},\"bridgeCalls\":{s},\"coreEvents\":{s},\"coreActions\":{s},\"runtimeSnapshots\":{s},\"testRuns\":{s},\"faultInjections\":{s}}}}}",
        .{ export_id, created_at, runtime_version, apps, app_versions, app_files, app_permissions, storage, app_migrations, install_reports, capabilities, runtime_sessions, bridge_calls, core_events, core_actions, runtime_snapshots, test_runs, fault_injections },
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

fn queryCoreActionsRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const sql = if (app_id == null)
        "SELECT action_id, event_id, session_id, app_id, action_json, created_at FROM core_actions ORDER BY created_at"
    else
        "SELECT action_id, event_id, session_id, app_id, action_json, created_at FROM core_actions WHERE app_id = ? ORDER BY created_at";
    return queryRowsJson(allocator, sql, app_id);
}

fn queryConsoleLogRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const sql = if (app_id == null)
        "SELECT bridge_call_id, session_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at"
    else
        "SELECT bridge_call_id, session_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at";
    return queryRowsJson(allocator, sql, app_id);
}

fn runtimeEventLogControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, app_id);
    defer allocator.free(bridge_calls);
    const core_events = try queryCoreEventsRowsJson(allocator, app_id);
    defer allocator.free(core_events);
    const core_actions = try queryCoreActionsRowsJson(allocator, app_id);
    defer allocator.free(core_actions);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.print(",\"bridgeCalls\":{s},\"coreEvents\":{s},\"coreActions\":{s}}}", .{ bridge_calls, core_events, core_actions });
    return out.toOwnedSlice();
}

fn consoleLogsControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const logs = try queryConsoleLogRowsJson(allocator, app_id);
    defer allocator.free(logs);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.print(",\"logs\":{s}}}", .{logs});
    return out.toOwnedSlice();
}

fn notificationCaptureControl(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    const sql = if (app_id == null)
        "SELECT app_id, params_json, created_at FROM bridge_calls WHERE method = 'notification.toast' ORDER BY created_at"
    else
        "SELECT app_id, params_json, created_at FROM bridge_calls WHERE method = 'notification.toast' AND app_id = ? ORDER BY created_at";
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (app_id) |actual_app_id| bindText(statement, 1, actual_app_id);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"appId\":");
    try appendJsonNullableString(allocator, &out, app_id);
    try out.writer.writeAll(",\"notifications\":[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const params_json = sqliteColumnNullableText(statement, 1) orelse "{}";
        var parsed = std.json.parseFromSlice(std.json.Value, allocator, params_json, .{}) catch null;
        defer if (parsed) |*actual| actual.deinit();
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{\"appId\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 0));
        try out.writer.writeAll(",\"message\":");
        if (parsed) |actual| {
            try appendJsonNullableString(allocator, &out, objectString(actual.value, "message"));
            try out.writer.writeAll(",\"level\":");
            try appendJsonNullableString(allocator, &out, objectString(actual.value, "level"));
        } else {
            try out.writer.writeAll("null,\"level\":null");
        }
        try out.writer.writeAll(",\"createdAt\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 2));
        try out.writer.print(",\"params\":{s}}}", .{params_json});
        count += 1;
    }
    try out.writer.writeAll("]}");
    return out.toOwnedSlice();
}

fn timerAdvanceControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const requested_ms = controlI64Arg(args, "ms") orelse controlI64Arg(args, "milliseconds") orelse 0;
    const advanced_ms = @max(requested_ms, 0);
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"advancedMs\":{d}}}", .{advanced_ms});
}

fn insertFaultInjectionControl(allocator: std.mem.Allocator, args: std.json.Value) ![]u8 {
    if (args != .object) return error.InvalidControlArgs;
    const method = objectString(args, "method") orelse blk: {
        const kind = objectString(args, "kind") orelse return error.InvalidControlArgs;
        break :blk methodForFaultKind(kind);
    };
    if (!isAllowedRuntimeBridgeMethod(method)) return error.UnknownBridgeMethod;
    const code = objectString(args, "code") orelse "fault_injected";
    const message = objectString(args, "message") orelse "Injected bridge fault";
    const once = if (args.object.get("once")) |once_value| if (once_value == .bool) once_value.bool else true else true;
    const details_json = if (args.object.get("details")) |details|
        try jsonValueAlloc(allocator, details)
    else
        try faultDetailsJsonAlloc(allocator, objectString(args, "kind"));
    defer allocator.free(details_json);

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const fault_id = try randomDbIdAlloc(allocator, db, "fault_");
    defer allocator.free(fault_id);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO fault_injections (fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, fault_id);
    bindNullableText(statement, 2, objectString(args, "sessionId"));
    bindNullableText(statement, 3, objectString(args, "appId"));
    bindText(statement, 4, method);
    bindText(statement, 5, code);
    bindText(statement, 6, message);
    bindText(statement, 7, details_json);
    _ = sqlite.sqlite3_bind_int64(statement, 8, if (once) 1 else 0);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;

    const escaped_fault_id = try escapeJsonString(allocator, fault_id);
    defer allocator.free(escaped_fault_id);
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print("{{\"ok\":true,\"faultId\":\"{s}\",\"appId\":", .{escaped_fault_id});
    try appendJsonNullableString(allocator, &out, objectString(args, "appId"));
    try out.writer.print(",\"method\":\"{s}\",\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{s},\"once\":{}}}", .{ escaped_method, escaped_code, escaped_message, details_json, once });
    return out.toOwnedSlice();
}

fn faultDetailsJsonAlloc(allocator: std.mem.Allocator, kind: ?[]const u8) ![]u8 {
    const actual_kind = kind orelse return allocator.dupe(u8, "{}");
    const escaped_kind = try escapeJsonString(allocator, actual_kind);
    defer allocator.free(escaped_kind);
    return std.fmt.allocPrint(allocator, "{{\"kind\":\"{s}\"}}", .{escaped_kind});
}

fn methodForFaultKind(kind: []const u8) []const u8 {
    if (std.mem.eql(u8, kind, "storage.read")) return "storage.get";
    if (std.mem.eql(u8, kind, "storage.write")) return "storage.set";
    if (std.mem.eql(u8, kind, "network") or std.mem.eql(u8, kind, "network.request")) return "network.request";
    if (std.mem.eql(u8, kind, "core") or std.mem.eql(u8, kind, "core.step")) return "core.step";
    return kind;
}

const FaultInjection = struct {
    fault_id: []u8,
    code: []u8,
    message: []u8,
    details_json: []u8,
};

fn freeFaultInjection(allocator: std.mem.Allocator, fault: FaultInjection) void {
    allocator.free(fault.fault_id);
    allocator.free(fault.code);
    allocator.free(fault.message);
    allocator.free(fault.details_json);
}

fn takeInjectedFaultAlloc(allocator: std.mem.Allocator, app_id: []const u8, session_id: ?[]const u8, method: []const u8) !?FaultInjection {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) ORDER BY created_at LIMIT 1",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, method);
    bindText(statement, 2, app_id);
    bindNullableText(statement, 3, session_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const fault_id = try allocator.dupe(u8, sqliteColumnText(statement, 0));
    errdefer allocator.free(fault_id);
    const code = try allocator.dupe(u8, sqliteColumnText(statement, 1));
    errdefer allocator.free(code);
    const message = try allocator.dupe(u8, sqliteColumnText(statement, 2));
    errdefer allocator.free(message);
    const details_json = try allocator.dupe(u8, sqliteColumnText(statement, 3));
    errdefer allocator.free(details_json);
    const once = sqlite.sqlite3_column_int64(statement, 4) != 0;
    if (once) try disableFaultInjection(db, fault_id);
    return .{ .fault_id = fault_id, .code = code, .message = message, .details_json = details_json };
}

fn disableFaultInjection(db: *sqlite.sqlite3, fault_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "UPDATE fault_injections SET enabled = 0 WHERE fault_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, fault_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn faultBridgeErrorJsonAlloc(allocator: std.mem.Allocator, fault: FaultInjection, app_id: []const u8, method: []const u8) ![]u8 {
    const escaped_code = try escapeJsonString(allocator, fault.code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, fault.message);
    defer allocator.free(escaped_message);
    const escaped_fault_id = try escapeJsonString(allocator, fault.fault_id);
    defer allocator.free(escaped_fault_id);
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    return std.fmt.allocPrint(
        allocator,
        "{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{{\"faultId\":\"{s}\",\"appId\":\"{s}\",\"method\":\"{s}\",\"injected\":{s}}}}}",
        .{ escaped_code, escaped_message, escaped_fault_id, escaped_app_id, escaped_method, fault.details_json },
    );
}

fn compareSnapshotControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const args_value = args orelse return error.InvalidControlArgs;
    if (args_value != .object) return error.InvalidControlArgs;
    const left_json = try snapshotCompareSideCanonicalJsonAlloc(allocator, args_value, "left", "leftSnapshotId");
    defer allocator.free(left_json);
    const right_json = try snapshotCompareSideCanonicalJsonAlloc(allocator, args_value, "right", "rightSnapshotId");
    defer allocator.free(right_json);
    const equal = std.mem.eql(u8, left_json, right_json);
    const left_hash = try sha256PrefixedAlloc(allocator, left_json);
    defer allocator.free(left_hash);
    const right_hash = try sha256PrefixedAlloc(allocator, right_json);
    defer allocator.free(right_hash);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":{},\"equal\":{},\"leftHash\":\"{s}\",\"rightHash\":\"{s}\"}}",
        .{ equal, equal, left_hash, right_hash },
    );
}

fn snapshotCompareSideCanonicalJsonAlloc(
    allocator: std.mem.Allocator,
    args: std.json.Value,
    inline_field: []const u8,
    snapshot_id_field: []const u8,
) ![]u8 {
    if (args.object.get(inline_field)) |inline_value| {
        if (inline_value != .null) {
            return canonicalJsonValueAlloc(allocator, inline_value);
        }
    }
    const snapshot_id = valueString(args.object.get(snapshot_id_field)) orelse return error.InvalidControlArgs;
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const snapshot_json = try snapshotJsonByIdAlloc(allocator, db, snapshot_id);
    defer allocator.free(snapshot_json);
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, snapshot_json, .{}) catch return error.SnapshotInvalid;
    defer parsed.deinit();
    return canonicalJsonValueAlloc(allocator, parsed.value);
}

fn canonicalJsonValueAlloc(allocator: std.mem.Allocator, value: std.json.Value) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try appendCanonicalJsonValue(allocator, &out, value);
    return out.toOwnedSlice();
}

fn appendCanonicalJsonValue(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, value: std.json.Value) !void {
    switch (value) {
        .null => try out.writer.writeAll("null"),
        .bool => |actual| try out.writer.writeAll(if (actual) "true" else "false"),
        .integer => |actual| try out.writer.print("{d}", .{actual}),
        .float => |actual| try out.writer.print("{d}", .{actual}),
        .number_string => |actual| try out.writer.writeAll(actual),
        .string => |actual| try appendJsonString(allocator, out, actual),
        .array => |array| {
            try out.writer.writeAll("[");
            for (array.items, 0..) |item, index| {
                if (index > 0) try out.writer.writeAll(",");
                try appendCanonicalJsonValue(allocator, out, item);
            }
            try out.writer.writeAll("]");
        },
        .object => |object| {
            var keys: std.ArrayList([]const u8) = .empty;
            defer keys.deinit(allocator);
            var iterator = object.iterator();
            while (iterator.next()) |entry| {
                try keys.append(allocator, entry.key_ptr.*);
            }
            std.mem.sort([]const u8, keys.items, {}, stringLessThan);
            try out.writer.writeAll("{");
            for (keys.items, 0..) |key, index| {
                if (index > 0) try out.writer.writeAll(",");
                try appendJsonString(allocator, out, key);
                try out.writer.writeAll(":");
                try appendCanonicalJsonValue(allocator, out, object.get(key).?);
            }
            try out.writer.writeAll("}");
        },
    }
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
        "SELECT session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, ended_at, status, capabilities_json, resource_high_water_json, metadata_json FROM runtime_sessions ORDER BY started_at",
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
        try out.writer.writeAll(",\"resource_high_water_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 10));
        try out.writer.writeAll(",\"metadata_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 11));
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
    host: []u8,
    path: []u8,
};

fn freeUrlParts(allocator: std.mem.Allocator, parts: UrlParts) void {
    allocator.free(parts.origin);
    allocator.free(parts.host);
    allocator.free(parts.path);
}

fn parseNetworkUrlAlloc(allocator: std.mem.Allocator, url: []const u8) !UrlParts {
    const scheme_end = std.mem.indexOf(u8, url, "://") orelse return error.InvalidNetworkUrl;
    if (scheme_end == 0) return error.InvalidNetworkUrl;
    const authority_start = scheme_end + 3;
    if (authority_start >= url.len) return error.InvalidNetworkUrl;
    const authority_end = urlAuthorityEnd(url, authority_start);
    if (authority_end == authority_start) return error.InvalidNetworkUrl;
    const authority = url[authority_start..authority_end];
    const origin = try allocator.dupe(u8, url[0..authority_end]);
    errdefer allocator.free(origin);
    const host = try networkHostFromAuthorityAlloc(allocator, authority);
    errdefer allocator.free(host);
    const path_part = if (authority_end < url.len and url[authority_end] == '/') url[authority_end..] else "/";
    const path_copy = try allocator.dupe(u8, path_part);
    return .{ .origin = origin, .host = host, .path = path_copy };
}

fn urlAuthorityEnd(url: []const u8, start: usize) usize {
    var index = start;
    while (index < url.len) : (index += 1) {
        switch (url[index]) {
            '/', '?', '#' => return index,
            else => {},
        }
    }
    return url.len;
}

fn networkHostFromAuthorityAlloc(allocator: std.mem.Allocator, authority: []const u8) ![]u8 {
    var host_port = authority;
    if (std.mem.lastIndexOfScalar(u8, host_port, '@')) |userinfo_end| {
        if (userinfo_end + 1 >= host_port.len) return error.InvalidNetworkUrl;
        host_port = host_port[userinfo_end + 1 ..];
    }
    if (host_port.len == 0) return error.InvalidNetworkUrl;
    if (host_port[0] == '[') {
        const bracket_end = std.mem.indexOfScalar(u8, host_port, ']') orelse return error.InvalidNetworkUrl;
        if (bracket_end <= 1) return error.InvalidNetworkUrl;
        return allocator.dupe(u8, host_port[1..bracket_end]);
    }
    const port_start = std.mem.indexOfScalar(u8, host_port, ':') orelse host_port.len;
    if (port_start == 0) return error.InvalidNetworkUrl;
    return allocator.dupe(u8, host_port[0..port_start]);
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
    if (networkPolicyDeniesPrivateNetwork(network_policy) and isPrivateNetworkHost(parts.host)) return false;
    const allow = network_policy.object.get("allow") orelse return false;
    if (allow != .array) return false;

    for (allow.array.items) |entry| {
        if (try networkPolicyEntryAllowsRequest(allocator, entry, parts, method, params)) return true;
    }
    return false;
}

fn networkPolicyDeniesPrivateNetwork(network_policy: std.json.Value) bool {
    if (network_policy != .object) return true;
    const value = network_policy.object.get("denyPrivateNetwork") orelse return true;
    if (value != .bool) return true;
    return value.bool;
}

fn isPrivateNetworkHost(host: []const u8) bool {
    if (std.ascii.eqlIgnoreCase(host, "localhost") or endsWithIgnoreCase(host, ".localhost")) return true;
    if (parseIpv4Host(host)) |octets| {
        return isPrivateIpv4Octets(octets);
    }
    return isPrivateIpv6Host(host);
}

fn isPrivateIpv4Octets(octets: [4]u8) bool {
    const first = octets[0];
    const second = octets[1];
    return first == 0 or
        first == 10 or
        first == 127 or
        (first == 100 and second >= 64 and second <= 127) or
        (first == 169 and second == 254) or
        (first == 172 and second >= 16 and second <= 31) or
        (first == 192 and second == 168);
}

fn parseIpv4Host(host: []const u8) ?[4]u8 {
    var octets: [4]u8 = undefined;
    var iterator = std.mem.splitScalar(u8, host, '.');
    var index: usize = 0;
    while (iterator.next()) |part| {
        if (index >= 4 or part.len == 0 or part.len > 3) return null;
        var value: u16 = 0;
        for (part) |char| {
            if (char < '0' or char > '9') return null;
            value = value * 10 + @as(u16, char - '0');
            if (value > 255) return null;
        }
        octets[index] = @as(u8, @intCast(value));
        index += 1;
    }
    if (index != 4) return null;
    return octets;
}

fn isPrivateIpv6Host(raw_host: []const u8) bool {
    var host = stripIpv6Brackets(raw_host);
    if (host.len == 0) return false;
    if (std.mem.indexOfScalar(u8, host, '%')) |zone_start| {
        host = host[0..zone_start];
    }
    if (std.ascii.eqlIgnoreCase(host, "::1")) return true;
    if (startsWithIgnoreCase(host, "fc") or startsWithIgnoreCase(host, "fd")) return true;
    if (startsWithIgnoreCase(host, "fe8") or startsWithIgnoreCase(host, "fe9") or startsWithIgnoreCase(host, "fea") or startsWithIgnoreCase(host, "feb")) return true;
    const mapped_prefix = "::ffff:";
    if (startsWithIgnoreCase(host, mapped_prefix)) {
        return isPrivateIpv4MappedHost(host[mapped_prefix.len..]);
    }
    return false;
}

fn isPrivateIpv4MappedHost(tail: []const u8) bool {
    if (parseIpv4Host(tail)) |octets| return isPrivateIpv4Octets(octets);
    var iterator = std.mem.splitScalar(u8, tail, ':');
    const high_text = iterator.next() orelse return false;
    const low_text = iterator.next() orelse return false;
    if (iterator.next() != null) return false;
    const high = parseHex16(high_text) orelse return false;
    const low = parseHex16(low_text) orelse return false;
    return isPrivateIpv4Octets(.{
        @as(u8, @intCast(high >> 8)),
        @as(u8, @intCast(high & 0x00ff)),
        @as(u8, @intCast(low >> 8)),
        @as(u8, @intCast(low & 0x00ff)),
    });
}

fn parseHex16(value: []const u8) ?u16 {
    if (value.len == 0 or value.len > 4) return null;
    var out: u16 = 0;
    for (value) |char| {
        const digit = hexDigitValue(char) orelse return null;
        out = out * 16 + digit;
    }
    return out;
}

fn hexDigitValue(char: u8) ?u16 {
    if (char >= '0' and char <= '9') return @as(u16, char - '0');
    if (char >= 'a' and char <= 'f') return @as(u16, char - 'a') + 10;
    if (char >= 'A' and char <= 'F') return @as(u16, char - 'A') + 10;
    return null;
}

fn stripIpv6Brackets(host: []const u8) []const u8 {
    if (host.len >= 2 and host[0] == '[' and host[host.len - 1] == ']') {
        return host[1 .. host.len - 1];
    }
    return host;
}

fn startsWithIgnoreCase(value: []const u8, prefix: []const u8) bool {
    if (value.len < prefix.len) return false;
    return std.ascii.eqlIgnoreCase(value[0..prefix.len], prefix);
}

fn endsWithIgnoreCase(value: []const u8, suffix: []const u8) bool {
    if (value.len < suffix.len) return false;
    return std.ascii.eqlIgnoreCase(value[value.len - suffix.len ..], suffix);
}

fn networkPolicyEntryAllowsRequest(
    allocator: std.mem.Allocator,
    entry: std.json.Value,
    parts: UrlParts,
    method: []const u8,
    params: std.json.Value,
) !bool {
    if (entry != .object) return false;
    const origin = valueString(entry.object.get("origin")) orelse return false;
    if (!std.mem.eql(u8, origin, parts.origin)) return false;
    const methods = entry.object.get("methods") orelse return false;
    if (!stringArrayContains(methods, method)) return false;
    if (entry.object.get("pathPrefix")) |path_prefix_value| {
        const path_prefix = valueString(path_prefix_value) orelse return false;
        if (!std.mem.startsWith(u8, parts.path, path_prefix)) return false;
    }
    if (params.object.get("credentials")) |credentials| {
        if (credentials != .null) return false;
    }
    if (!headersAllowed(params.object.get("headers"), entry.object.get("allowedHeaders"))) return false;
    if (!(try requestBodyAllowed(allocator, params.object.get("body"), entry.object.get("maxRequestBytes")))) return false;
    return true;
}

fn activeManifestJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    return activeManifestJsonInDbAlloc(allocator, db, app_id);
}

fn activeManifestJsonInDbAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) ![]u8 {
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

fn assertServerRequiredCapabilitiesAvailable(allocator: std.mem.Allocator, manifest_json: []const u8) !void {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{}) catch return error.InvalidWebappPackage;
    defer parsed.deinit();
    if (parsed.value != .object) return error.InvalidWebappPackage;
    const capabilities = parsed.value.object.get("capabilities") orelse return error.InvalidWebappPackage;
    if (capabilities != .object) return error.InvalidWebappPackage;
    const required = capabilities.object.get("required") orelse return error.InvalidWebappPackage;
    if (required != .array) return error.InvalidWebappPackage;
    for (required.array.items) |capability| {
        const capability_name = valueString(capability) orelse return error.InvalidWebappPackage;
        if (!serverCapabilityAvailable(capability_name)) return error.CapabilityUnavailable;
    }
}

fn serverCapabilityAvailable(capability: []const u8) bool {
    const capabilities = [_][]const u8{
        "core.step",
        "storage.read",
        "storage.write",
        "dialog.openFile",
        "dialog.saveFile",
        "notification.toast",
        "network.request",
        "app.log",
        "runtime.capabilities",
        "runtime.snapshot",
        "runtime.replay",
    };
    for (capabilities) |candidate| {
        if (std.mem.eql(u8, capability, candidate)) return true;
    }
    return false;
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
        if (isCredentialHeader(entry.key_ptr.*)) return false;
        if (!stringArrayContains(allowed, entry.key_ptr.*)) return false;
    }
    return true;
}

fn networkRequestUsesCredentials(params: std.json.Value) bool {
    if (params.object.get("credentials")) |credentials| {
        if (credentials != .null) return true;
    }
    if (params.object.get("headers")) |headers| {
        if (headers != .object) return false;
        var iterator = headers.object.iterator();
        while (iterator.next()) |entry| {
            if (isCredentialHeader(entry.key_ptr.*)) return true;
        }
    }
    return false;
}

fn isCredentialHeader(name: []const u8) bool {
    return std.ascii.eqlIgnoreCase(name, "cookie") or std.ascii.eqlIgnoreCase(name, "set-cookie");
}

fn requestBodyAllowed(allocator: std.mem.Allocator, body_value: ?std.json.Value, max_value: ?std.json.Value) !bool {
    const max = max_value orelse return true;
    if (max != .integer) return false;
    const body = body_value orelse return true;
    if (body == .null) return true;
    if (body == .string) return body.string.len <= @as(usize, @intCast(max.integer));
    const body_json = try jsonValueAlloc(allocator, body);
    defer allocator.free(body_json);
    return body_json.len <= @as(usize, @intCast(max.integer));
}

const NetworkPolicyBridgeError = struct {
    code: []const u8,
    message: []const u8,
    details_json: []u8,
};

fn freeNetworkPolicyBridgeError(allocator: std.mem.Allocator, policy_error: NetworkPolicyBridgeError) void {
    allocator.free(policy_error.details_json);
}

fn networkRequestDenyDetailsJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    url: []const u8,
    method: []const u8,
    params: std.json.Value,
) !?[]u8 {
    const parts = try parseNetworkUrlAlloc(allocator, url);
    defer freeUrlParts(allocator, parts);

    const manifest_json = try activeManifestJsonAlloc(allocator, app_id);
    defer allocator.free(manifest_json);
    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{});
    defer parsed.deinit();

    const network_policy = parsed.value.object.get("networkPolicy") orelse return try networkOriginMethodDetailsJsonAlloc(allocator, parts.origin, method);
    if (network_policy != .object) return try networkOriginMethodDetailsJsonAlloc(allocator, parts.origin, method);
    if (networkPolicyDeniesPrivateNetwork(network_policy) and isPrivateNetworkHost(parts.host)) {
        return try networkOriginHostDetailsJsonAlloc(allocator, parts.origin, parts.host);
    }
    const allow = network_policy.object.get("allow") orelse return try networkOriginMethodDetailsJsonAlloc(allocator, parts.origin, method);
    if (allow != .array) return try networkOriginMethodDetailsJsonAlloc(allocator, parts.origin, method);

    const entry = networkPolicyMatchingEntry(allow, parts, method) orelse {
        return try networkOriginMethodDetailsJsonAlloc(allocator, parts.origin, method);
    };
    if (try networkHeaderViolationDetailsJsonAlloc(allocator, params.object.get("headers"), entry.object.get("allowedHeaders"))) |details| {
        return details;
    }
    if (try networkCredentialsViolationDetailsJsonAlloc(allocator, params.object.get("credentials"))) |details| {
        return details;
    }
    if (try networkRequestBodyViolationDetailsJsonAlloc(allocator, params.object.get("body"), entry.object.get("maxRequestBytes"))) |details| {
        return details;
    }
    return null;
}

fn networkResponsePolicyErrorAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    url: []const u8,
    method: []const u8,
    params: std.json.Value,
    response_json: []const u8,
) !?NetworkPolicyBridgeError {
    var response = std.json.parseFromSlice(std.json.Value, allocator, response_json, .{}) catch return null;
    defer response.deinit();
    if (response.value != .object) return null;

    const manifest_json = try activeManifestJsonAlloc(allocator, app_id);
    defer allocator.free(manifest_json);
    var parsed_manifest = try std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{});
    defer parsed_manifest.deinit();

    const parts = try parseNetworkUrlAlloc(allocator, url);
    defer freeUrlParts(allocator, parts);
    const network_policy = parsed_manifest.value.object.get("networkPolicy") orelse return null;
    if (network_policy != .object) return null;
    const allow = network_policy.object.get("allow") orelse return null;
    if (allow != .array) return null;
    const entry = networkPolicyMatchingEntry(allow, parts, method) orelse return null;

    if (valueI64(response.value.object.get("delayMs"))) |delay_ms| {
        if (networkEffectiveTimeoutMs(params.object.get("timeoutMs"), entry.object.get("timeoutMs"))) |timeout_ms| {
            if (delay_ms > timeout_ms) {
                return .{
                    .code = "timeout",
                    .message = "network.request timed out",
                    .details_json = try networkTimeoutDetailsJsonAlloc(allocator, timeout_ms, delay_ms),
                };
            }
        }
    }

    if (networkEffectiveResponseBytesLimit(parsed_manifest.value, entry)) |limit| {
        const bytes = try jsonPayloadBytes(allocator, response.value.object.get("bodyText") orelse response.value.object.get("body"));
        if (bytes > limit) {
            return .{
                .code = "network_policy_denied",
                .message = "network.response exceeds allowed byte limit",
                .details_json = try networkMaxBytesDetailsJsonAlloc(allocator, "maxResponseBytes", limit, bytes),
            };
        }
    }

    const status = valueI64(response.value.object.get("status")) orelse 0;
    if (status >= 300 and status < 400) {
        if (networkHeaderString(response.value.object.get("headers"), "location")) |location| {
            const redirect_parts = parseNetworkUrlAlloc(allocator, location) catch {
                return .{
                    .code = "network_policy_denied",
                    .message = "network.response redirect location is invalid",
                    .details_json = try networkLocationDetailsJsonAlloc(allocator, location),
                };
            };
            defer freeUrlParts(allocator, redirect_parts);
            if (networkPolicyDeniesPrivateNetwork(network_policy) and isPrivateNetworkHost(redirect_parts.host)) {
                return .{
                    .code = "network_policy_denied",
                    .message = "network.response redirect targets private network",
                    .details_json = try networkOriginHostDetailsJsonAlloc(allocator, redirect_parts.origin, redirect_parts.host),
                };
            }
            if (networkPolicyMatchingEntry(allow, redirect_parts, method) == null) {
                return .{
                    .code = "network_policy_denied",
                    .message = "network.response redirect is outside manifest.networkPolicy",
                    .details_json = try networkOriginMethodDetailsJsonAlloc(allocator, redirect_parts.origin, method),
                };
            }
        }
    }

    return null;
}

fn networkResponsePayloadJsonAlloc(allocator: std.mem.Allocator, response_json: []const u8) ![]u8 {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, response_json, .{}) catch {
        return allocator.dupe(u8, response_json);
    };
    defer parsed.deinit();
    if (parsed.value != .object or parsed.value.object.get("delayMs") == null) {
        return allocator.dupe(u8, response_json);
    }

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeByte('{');
    var first = true;
    var iterator = parsed.value.object.iterator();
    while (iterator.next()) |entry| {
        if (std.mem.eql(u8, entry.key_ptr.*, "delayMs")) continue;
        if (!first) try out.writer.writeByte(',');
        first = false;
        try std.json.Stringify.value(entry.key_ptr.*, .{}, &out.writer);
        try out.writer.writeByte(':');
        try std.json.Stringify.value(entry.value_ptr.*, .{}, &out.writer);
    }
    try out.writer.writeByte('}');
    return out.toOwnedSlice();
}

fn networkPolicyMatchingEntry(allow: std.json.Value, parts: UrlParts, method: []const u8) ?std.json.Value {
    if (allow != .array) return null;
    for (allow.array.items) |entry| {
        if (networkPolicyEntryMatchesRoute(entry, parts, method)) return entry;
    }
    return null;
}

fn networkPolicyEntryMatchesRoute(entry: std.json.Value, parts: UrlParts, method: []const u8) bool {
    if (entry != .object) return false;
    const origin = valueString(entry.object.get("origin")) orelse return false;
    if (!std.mem.eql(u8, origin, parts.origin)) return false;
    const methods = entry.object.get("methods") orelse return false;
    if (!stringArrayContains(methods, method)) return false;
    if (entry.object.get("pathPrefix")) |path_prefix_value| {
        const path_prefix = valueString(path_prefix_value) orelse return false;
        if (!std.mem.startsWith(u8, parts.path, path_prefix)) return false;
    }
    return true;
}

fn networkHeaderViolationDetailsJsonAlloc(
    allocator: std.mem.Allocator,
    headers_value: ?std.json.Value,
    allowed_value: ?std.json.Value,
) !?[]u8 {
    const headers = headers_value orelse return null;
    if (headers == .null) return null;
    if (headers != .object) return null;
    const allowed = allowed_value orelse {
        var iterator = headers.object.iterator();
        if (iterator.next()) |entry| return try networkHeaderDetailsJsonAlloc(allocator, entry.key_ptr.*);
        return null;
    };
    if (allowed != .array) return null;

    var iterator = headers.object.iterator();
    while (iterator.next()) |entry| {
        if (isCredentialHeader(entry.key_ptr.*)) return try networkHeaderDetailsJsonAlloc(allocator, entry.key_ptr.*);
        if (!stringArrayContains(allowed, entry.key_ptr.*)) return try networkHeaderDetailsJsonAlloc(allocator, entry.key_ptr.*);
    }
    return null;
}

fn networkCredentialsViolationDetailsJsonAlloc(allocator: std.mem.Allocator, credentials_value: ?std.json.Value) !?[]u8 {
    const credentials = credentials_value orelse return null;
    if (credentials == .null) return null;
    const credentials_json = try jsonValueAlloc(allocator, credentials);
    defer allocator.free(credentials_json);
    return try std.fmt.allocPrint(allocator, "{{\"credentials\":{s}}}", .{credentials_json});
}

fn networkRequestBodyViolationDetailsJsonAlloc(
    allocator: std.mem.Allocator,
    body_value: ?std.json.Value,
    max_value: ?std.json.Value,
) !?[]u8 {
    const max = valueI64(max_value) orelse return null;
    if (max < 0) return null;
    const bytes = try jsonPayloadBytes(allocator, body_value);
    if (bytes <= max) return null;
    return try networkMaxBytesDetailsJsonAlloc(allocator, "maxRequestBytes", max, bytes);
}

fn networkEffectiveTimeoutMs(requested_value: ?std.json.Value, policy_value: ?std.json.Value) ?i64 {
    const requested = positiveI64(requested_value);
    const policy = positiveI64(policy_value);
    if (requested != null and policy != null) return @min(requested.?, policy.?);
    return requested orelse policy;
}

fn networkEffectiveResponseBytesLimit(manifest: std.json.Value, entry: std.json.Value) ?i64 {
    const policy_limit = positiveI64(entry.object.get("maxResponseBytes"));
    const budget_limit = resourceBudgetLimit(manifest, "maxNetworkResponseBytes");
    if (policy_limit != null and budget_limit != null) return @min(policy_limit.?, budget_limit.?);
    return policy_limit orelse budget_limit;
}

fn positiveI64(value: ?std.json.Value) ?i64 {
    const actual = valueI64(value) orelse return null;
    if (actual <= 0) return null;
    return actual;
}

fn jsonPayloadBytes(allocator: std.mem.Allocator, value_opt: ?std.json.Value) !i64 {
    const value = value_opt orelse return 0;
    if (value == .null) return 0;
    if (value == .string) return @as(i64, @intCast(value.string.len));
    const json = try jsonValueAlloc(allocator, value);
    defer allocator.free(json);
    return @as(i64, @intCast(json.len));
}

fn networkHeaderString(headers_value: ?std.json.Value, name: []const u8) ?[]const u8 {
    const headers = headers_value orelse return null;
    if (headers != .object) return null;
    var iterator = headers.object.iterator();
    while (iterator.next()) |entry| {
        if (std.ascii.eqlIgnoreCase(entry.key_ptr.*, name)) {
            return valueString(entry.value_ptr.*);
        }
    }
    return null;
}

fn networkOriginMethodDetailsJsonAlloc(allocator: std.mem.Allocator, origin: []const u8, method: []const u8) ![]u8 {
    const escaped_origin = try escapeJsonString(allocator, origin);
    defer allocator.free(escaped_origin);
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    return std.fmt.allocPrint(allocator, "{{\"origin\":\"{s}\",\"method\":\"{s}\"}}", .{ escaped_origin, escaped_method });
}

fn networkOriginHostDetailsJsonAlloc(allocator: std.mem.Allocator, origin: []const u8, host: []const u8) ![]u8 {
    const escaped_origin = try escapeJsonString(allocator, origin);
    defer allocator.free(escaped_origin);
    const escaped_host = try escapeJsonString(allocator, host);
    defer allocator.free(escaped_host);
    return std.fmt.allocPrint(allocator, "{{\"origin\":\"{s}\",\"host\":\"{s}\"}}", .{ escaped_origin, escaped_host });
}

fn networkHeaderDetailsJsonAlloc(allocator: std.mem.Allocator, header: []const u8) ![]u8 {
    const escaped_header = try escapeJsonString(allocator, header);
    defer allocator.free(escaped_header);
    return std.fmt.allocPrint(allocator, "{{\"header\":\"{s}\"}}", .{escaped_header});
}

fn networkMaxBytesDetailsJsonAlloc(allocator: std.mem.Allocator, field: []const u8, max: i64, bytes: i64) ![]u8 {
    const escaped_field = try escapeJsonString(allocator, field);
    defer allocator.free(escaped_field);
    return std.fmt.allocPrint(allocator, "{{\"{s}\":{d},\"bytes\":{d}}}", .{ escaped_field, max, bytes });
}

fn networkTimeoutDetailsJsonAlloc(allocator: std.mem.Allocator, timeout_ms: i64, delay_ms: i64) ![]u8 {
    return std.fmt.allocPrint(allocator, "{{\"timeoutMs\":{d},\"delayMs\":{d}}}", .{ timeout_ms, delay_ms });
}

fn networkUrlDetailsJsonAlloc(allocator: std.mem.Allocator, url: []const u8) ![]u8 {
    const escaped_url = try escapeJsonString(allocator, url);
    defer allocator.free(escaped_url);
    return std.fmt.allocPrint(allocator, "{{\"url\":\"{s}\"}}", .{escaped_url});
}

fn networkLocationDetailsJsonAlloc(allocator: std.mem.Allocator, location: []const u8) ![]u8 {
    const escaped_location = try escapeJsonString(allocator, location);
    defer allocator.free(escaped_location);
    return std.fmt.allocPrint(allocator, "{{\"location\":\"{s}\"}}", .{escaped_location});
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

fn dialogMockResultJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    dialog_type: []const u8,
) !?[]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT response_json FROM dialog_mocks WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) ORDER BY created_at DESC LIMIT 1",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, dialog_type);
    bindText(statement, 2, app_id);
    bindNullableText(statement, 3, session_id);

    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const response_copy = try allocator.dupe(u8, sqliteColumnText(statement, 0));
    return response_copy;
}

fn insertDialogMockControl(allocator: std.mem.Allocator, args: std.json.Value) ![]u8 {
    if (args != .object) return error.InvalidControlArgs;
    const app_id = controlStringArg(args, "appId");
    const session_id = controlStringArg(args, "sessionId");
    const dialog_type_raw = controlStringArg(args, "dialogType") orelse blk: {
        const method = controlStringArg(args, "method") orelse return error.InvalidControlArgs;
        break :blk dialogTypeForBridgeMethod(method) orelse return error.InvalidControlArgs;
    };
    const dialog_type = normalizeDialogType(dialog_type_raw) orelse return error.InvalidControlArgs;
    const response_json = if (args.object.get("response")) |response|
        try jsonValueAlloc(allocator, response)
    else
        try defaultDialogResponseJsonAlloc(allocator, dialog_type, args);
    defer allocator.free(response_json);

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at) VALUES ('dialogmock_' || lower(hex(randomblob(16))), ?, ?, ?, ?, 1, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindNullableText(statement, 1, session_id);
    bindNullableText(statement, 2, app_id);
    bindText(statement, 3, dialog_type);
    bindText(statement, 4, response_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"dialogType\":\"{s}\"}}", .{dialog_type});
}

fn defaultDialogResponseJsonAlloc(allocator: std.mem.Allocator, dialog_type: []const u8, args: std.json.Value) ![]u8 {
    if (std.mem.eql(u8, dialog_type, "saveFile")) {
        return allocator.dupe(u8, "{\"ok\":true}");
    }
    if (std.mem.eql(u8, dialog_type, "openFile")) {
        if (args.object.get("files")) |files| {
            const files_json = try jsonValueAlloc(allocator, files);
            defer allocator.free(files_json);
            return std.fmt.allocPrint(allocator, "{{\"files\":{s}}}", .{files_json});
        }
        return allocator.dupe(u8, "{\"files\":[]}");
    }
    return error.InvalidControlArgs;
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

fn permissionDetailsJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8, method: []const u8, permission: []const u8) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    const escaped_permission = try escapeJsonString(allocator, permission);
    defer allocator.free(escaped_permission);
    return std.fmt.allocPrint(
        allocator,
        "{{\"appId\":\"{s}\",\"method\":\"{s}\",\"requiredPermission\":\"{s}\"}}",
        .{ escaped_app_id, escaped_method, escaped_permission },
    );
}

fn methodDetailsJsonAlloc(allocator: std.mem.Allocator, method: []const u8) ![]u8 {
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    return std.fmt.allocPrint(allocator, "{{\"method\":\"{s}\"}}", .{escaped_method});
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
    const active_install_id = try activeInstallIdAlloc(allocator, db, app_id);
    defer if (active_install_id) |install_id| allocator.free(install_id);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) " ++
            "VALUES ('bridge_' || lower(hex(randomblob(16))), ?, ?, ?, ?, ?, ?, ?, 0, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, actual_session_id);
    bindText(statement, 2, app_id);
    bindNullableText(statement, 3, active_install_id);
    bindText(statement, 4, method);
    bindText(statement, 5, params_json);
    bindNullableText(statement, 6, result_json);
    bindNullableText(statement, 7, error_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
    const usage_json = try snapshotResourceUsageJsonAlloc(allocator, db, app_id);
    defer allocator.free(usage_json);
    try updateRuntimeSessionResourceHighWater(db, actual_session_id, usage_json);
    if (active_install_id) |install_id| {
        maybeQuarantineAfterBudgetError(allocator, app_id, install_id, error_json) catch |err| {
            std.debug.print("budget quarantine check failed: {}\n", .{err});
        };
    }
}

fn maybeQuarantineAfterBudgetError(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    install_id: []const u8,
    error_json: ?[]const u8,
) !void {
    const raw_error = error_json orelse return;
    if (std.mem.indexOf(u8, raw_error, "\"code\":\"resource_budget_exceeded\"") == null) return;

    const count = try countBridgeErrorsSince(allocator, app_id, install_id, "resource_budget_exceeded");
    if (count < 3) return;

    const active_install_id = try activeInstallIdForAppAlloc(allocator, app_id);
    defer if (active_install_id) |active| allocator.free(active);
    const active = active_install_id orelse return;
    if (!std.mem.eql(u8, active, install_id)) return;

    const result_json = try quarantineWebappPackage(allocator, app_id, install_id, "resource_budget_exceeded", true, "zig-server-runtime");
    defer allocator.free(result_json);
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
    const usage_json = try snapshotResourceUsageJsonAlloc(allocator, db, app_id);
    defer allocator.free(usage_json);
    try updateRuntimeSessionResourceHighWater(db, actual_session_id, usage_json);
}

fn updateRuntimeSessionResourceHighWater(db: *sqlite.sqlite3, session_id: []const u8, usage_json: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE runtime_sessions SET resource_high_water_json = ? WHERE session_id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, usage_json);
    bindText(statement, 2, session_id);
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
        "INSERT OR IGNORE INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) " ++
            "VALUES (?, 'zig-server', 'server', ?, ?, datetime('now'), 'running', NULL, '{\"storageBytes\":0,\"bridgeCalls\":0,\"coreEvents\":0}', '{\"source\":\"bridge\"}')",
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
        403 => "Forbidden",
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

fn bridgeOkJsonAlloc(allocator: std.mem.Allocator, id: []const u8, result_json: []const u8) ![]u8 {
    const escaped_id = try escapeJsonString(allocator, id);
    defer allocator.free(escaped_id);
    return std.fmt.allocPrint(allocator, "{{\"id\":\"{s}\",\"ok\":true,\"result\":{s}}}", .{ escaped_id, result_json });
}

fn writeBridgeError(allocator: std.mem.Allocator, stream: std.net.Stream, id: []const u8, code: []const u8, message: []const u8) !void {
    return writeBridgeErrorWithDetails(allocator, stream, id, code, message, "{}");
}

fn writeBridgeErrorWithDetails(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    code: []const u8,
    message: []const u8,
    details_json: []const u8,
) !void {
    const escaped_id = try escapeJsonString(allocator, id);
    defer allocator.free(escaped_id);
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    const body = try std.fmt.allocPrint(
        allocator,
        "{{\"id\":\"{s}\",\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{s}}}}}",
        .{ escaped_id, escaped_code, escaped_message, details_json },
    );
    defer allocator.free(body);
    return writeJson(stream, 200, body);
}

fn bridgeErrorResponseJsonAlloc(allocator: std.mem.Allocator, id: []const u8, code: []const u8, message: []const u8) ![]u8 {
    return bridgeErrorResponseJsonWithDetailsAlloc(allocator, id, code, message, "{}");
}

fn bridgeErrorResponseJsonWithDetailsAlloc(
    allocator: std.mem.Allocator,
    id: []const u8,
    code: []const u8,
    message: []const u8,
    details_json: []const u8,
) ![]u8 {
    const escaped_id = try escapeJsonString(allocator, id);
    defer allocator.free(escaped_id);
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    return std.fmt.allocPrint(
        allocator,
        "{{\"id\":\"{s}\",\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{s}}}}}",
        .{ escaped_id, escaped_code, escaped_message, details_json },
    );
}

fn bridgeControlErrorResponse(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params_json: []const u8,
    code: []const u8,
    message: []const u8,
) ![]u8 {
    return bridgeControlErrorResponseWithDetails(allocator, app_id, session_id, id, method, params_json, code, message, "{}");
}

fn bridgeControlErrorResponseWithDetails(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    id: []const u8,
    method: []const u8,
    params_json: []const u8,
    code: []const u8,
    message: []const u8,
    details_json: []const u8,
) ![]u8 {
    const error_json = try bridgeErrorJsonWithDetailsAlloc(allocator, code, message, details_json);
    defer allocator.free(error_json);
    logBridgeCall(allocator, app_id, session_id, method, params_json, null, error_json) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return bridgeErrorResponseJsonWithDetailsAlloc(allocator, id, code, message, details_json);
}

fn bridgeErrorJsonAlloc(allocator: std.mem.Allocator, code: []const u8, message: []const u8) ![]u8 {
    return bridgeErrorJsonWithDetailsAlloc(allocator, code, message, "{}");
}

fn bridgeErrorJsonWithDetailsAlloc(allocator: std.mem.Allocator, code: []const u8, message: []const u8, details_json: []const u8) ![]u8 {
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    return std.fmt.allocPrint(
        allocator,
        "{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{s}}}",
        .{ escaped_code, escaped_message, details_json },
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

fn writeControlBanError(allocator: std.mem.Allocator, stream: std.net.Stream, retry_after_seconds: i64) !void {
    const body = try std.fmt.allocPrint(
        allocator,
        "{{\"ok\":false,\"error\":{{\"code\":\"control_connection_banned\",\"message\":\"Control connection is temporarily banned after repeated auth failures\",\"details\":{{\"retryAfterSeconds\":{d}}}}}}}",
        .{retry_after_seconds},
    );
    defer allocator.free(body);
    return writeJson(stream, 403, body);
}

fn serverCapabilitiesJson(allocator: std.mem.Allocator) ![]u8 {
    return serverCapabilitiesForAppOptionalJson(allocator, null);
}

fn serverCapabilitiesForAppJson(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    return serverCapabilitiesForAppOptionalJson(allocator, app_id);
}

fn serverCapabilitiesForAppOptionalJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const app_id_json = if (app_id) |actual_app_id| blk: {
        const escaped_app_id = try escapeJsonString(allocator, actual_app_id);
        defer allocator.free(escaped_app_id);
        break :blk try std.fmt.allocPrint(allocator, ",\"appId\":\"{s}\"", .{escaped_app_id});
    } else try allocator.dupe(u8, "");
    defer allocator.free(app_id_json);
    return std.fmt.allocPrint(
        allocator,
        "{{\"runtimeVersion\":\"{s}\",\"platform\":\"server\",\"target\":\"zig-server\",\"devMode\":false,\"features\":{{\"core.step\":true,\"runtime.capabilities\":true,\"runtime.snapshot\":true,\"runtime.replay\":true,\"storage.read\":true,\"storage.write\":true,\"storage.get\":true,\"storage.set\":true,\"storage.remove\":true,\"storage.list\":true,\"dialog.openFile\":true,\"dialog.saveFile\":true,\"notification.toast\":true,\"network.request\":true,\"app.log\":true}},\"limits\":{{\"maxBodyBytes\":1048576,\"maxStorageBytes\":5242880,\"maxBridgeCallsPerMinute\":600,\"maxPackageBytes\":1048576,\"maxFileBytes\":524288}}{s}}}",
        .{ runtime_version, app_id_json },
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

fn isKnownPackagePermission(permission: []const u8) bool {
    const permissions = [_][]const u8{
        "core.step",
        "storage.read",
        "storage.write",
        "dialog.openFile",
        "dialog.saveFile",
        "notification.toast",
        "network.request",
        "app.log",
    };
    for (permissions) |candidate| {
        if (std.mem.eql(u8, permission, candidate)) return true;
    }
    return false;
}

fn validateServerCapabilities(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    capabilities: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (capabilities != .object) {
        try errors.append(allocator, "invalid_capabilities");
        return;
    }
    const required_fields = [_][]const u8{ "required", "optional" };
    for (required_fields) |field| {
        const value = capabilities.object.get(field) orelse {
            try errors.append(allocator, "invalid_capabilities");
            continue;
        };
        if (value != .array) {
            try errors.append(allocator, "invalid_capabilities");
            continue;
        }
        for (value.array.items) |capability| {
            const capability_name = valueString(capability) orelse {
                try errors.append(allocator, "invalid_capabilities");
                continue;
            };
            if (!std.mem.startsWith(u8, capability_name, "runtime.") and !manifestPermissionsContain(manifest, capability_name)) {
                try errors.append(allocator, "invalid_capabilities");
            }
        }
    }
}

fn manifestTrustLevelIs(manifest: std.json.Value, expected: []const u8) bool {
    if (manifest != .object) return false;
    const trust = manifest.object.get("trust") orelse return false;
    if (trust != .object) return false;
    const level = valueString(trust.object.get("level")) orelse return false;
    return std.mem.eql(u8, level, expected);
}

fn validateServerContentRating(
    allocator: std.mem.Allocator,
    content_rating: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (content_rating != .object) {
        try errors.append(allocator, "invalid_content_rating");
        return;
    }

    const required_keys = [_][]const u8{ "scheme", "label", "minimumAge", "descriptors" };
    for (required_keys) |key| {
        if (content_rating.object.get(key) == null) {
            try errors.append(allocator, "invalid_content_rating");
        }
    }

    const scheme = valueString(content_rating.object.get("scheme"));
    if (scheme == null or !std.mem.eql(u8, scheme.?, "app-store")) {
        try errors.append(allocator, "invalid_content_rating");
    }

    const label = valueString(content_rating.object.get("label"));
    const expected_minimum_age = if (label) |actual| contentRatingMinimumAge(actual) else null;
    if (expected_minimum_age == null) {
        try errors.append(allocator, "invalid_content_rating");
    } else {
        const minimum_age = valueI64(content_rating.object.get("minimumAge"));
        if (minimum_age == null or minimum_age.? != expected_minimum_age.?) {
            try errors.append(allocator, "invalid_content_rating");
        }
    }

    const descriptors = content_rating.object.get("descriptors");
    if (descriptors == null or descriptors.? != .array) {
        try errors.append(allocator, "invalid_content_rating");
        return;
    }
    for (descriptors.?.array.items, 0..) |descriptor, index| {
        const actual = valueString(descriptor) orelse {
            try errors.append(allocator, "invalid_content_rating");
            continue;
        };
        for (descriptors.?.array.items[0..index]) |previous| {
            if (valueString(previous)) |previous_actual| {
                if (std.mem.eql(u8, actual, previous_actual)) {
                    try errors.append(allocator, "invalid_content_rating");
                    break;
                }
            }
        }
    }
}

fn contentRatingMinimumAge(label: []const u8) ?i64 {
    if (std.mem.eql(u8, label, "4+")) return 4;
    if (std.mem.eql(u8, label, "9+")) return 9;
    if (std.mem.eql(u8, label, "12+")) return 12;
    if (std.mem.eql(u8, label, "17+")) return 17;
    return null;
}

fn manifestPermissionsContain(manifest: std.json.Value, permission: []const u8) bool {
    if (manifest != .object) return false;
    const permissions = manifest.object.get("permissions") orelse return false;
    if (permissions != .array) return false;
    for (permissions.array.items) |candidate| {
        if (valueString(candidate)) |actual| {
            if (std.mem.eql(u8, actual, permission)) return true;
        }
    }
    return false;
}

fn validateServerResourceBudget(
    allocator: std.mem.Allocator,
    resource_budget: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (resource_budget != .object) {
        try errors.append(allocator, "invalid_resource_budget");
        return;
    }
    const required_keys = [_][]const u8{
        "maxDomNodes",
        "maxStorageBytes",
        "maxBridgeCallsPerMinute",
        "maxNetworkRequestsPerMinute",
        "maxTimers",
        "maxLogLinesPerMinute",
        "maxPackageBytes",
        "maxFileBytes",
    };
    for (required_keys) |key| {
        const limit = valueI64(resource_budget.object.get(key)) orelse {
            try errors.append(allocator, "invalid_resource_budget");
            continue;
        };
        if (limit < 0) try errors.append(allocator, "invalid_resource_budget");
    }
}

fn validateServerPackageBudget(
    allocator: std.mem.Allocator,
    files: std.json.Value,
    resource_budget: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (files != .array or resource_budget != .object) return;

    const max_file_bytes = valueI64(resource_budget.object.get("maxFileBytes"));
    const max_package_bytes = valueI64(resource_budget.object.get("maxPackageBytes"));
    var package_bytes: i64 = 0;
    for (files.array.items) |file| {
        if (file != .object) continue;
        const content = valueString(file.object.get("content")) orelse continue;
        const file_bytes = @as(i64, @intCast(content.len));
        package_bytes += file_bytes;
        if (max_file_bytes) |limit| {
            if (limit >= 0 and file_bytes > limit) try errors.append(allocator, "resource_budget_exceeded");
        }
    }
    if (max_package_bytes) |limit| {
        if (limit >= 0 and package_bytes > limit) try errors.append(allocator, "resource_budget_exceeded");
    }
}

fn validateServerMigrations(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    files: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (files != .array) return;
    const app_id = valueString(manifest.object.get("id")) orelse return;
    const storage_prefix = valueString(manifest.object.get("storagePrefix")) orelse return;
    const data_version = valueI64(manifest.object.get("dataVersion")) orelse return;
    if (data_version < 1) return;

    var from: i64 = 1;
    while (from < data_version) : (from += 1) {
        const migration_path = try std.fmt.allocPrint(allocator, "migrations/{d}_to_{d}.json", .{ from, from + 1 });
        defer allocator.free(migration_path);
        if (findPackageFile(files, migration_path) == null) try errors.append(allocator, "migration_missing");
    }

    for (files.array.items) |file| {
        if (file != .object) continue;
        const path = valueString(file.object.get("path")) orelse continue;
        if (!std.mem.startsWith(u8, path, "migrations/") or !std.mem.endsWith(u8, path, ".json")) continue;

        const version = parseMigrationFileVersion(path) orelse {
            try errors.append(allocator, "invalid_migration_filename");
            continue;
        };
        const content = valueString(file.object.get("content")) orelse continue;
        var parsed = std.json.parseFromSlice(std.json.Value, allocator, content, .{}) catch {
            try errors.append(allocator, "invalid_migration_json");
            continue;
        };
        defer parsed.deinit();
        if (parsed.value != .object) {
            try errors.append(allocator, "invalid_migration");
            continue;
        }

        const migration_app_id = valueString(parsed.value.object.get("appId"));
        if (migration_app_id == null or !std.mem.eql(u8, migration_app_id.?, app_id)) {
            try errors.append(allocator, "invalid_migration_app");
        }
        const from_data_version = valueI64(parsed.value.object.get("fromDataVersion"));
        const to_data_version = valueI64(parsed.value.object.get("toDataVersion"));
        if (from_data_version == null or to_data_version == null or
            from_data_version.? != version.from or
            to_data_version.? != version.to or
            version.to != version.from + 1)
        {
            try errors.append(allocator, "invalid_migration_version");
        }
        try validateServerMigrationSteps(allocator, storage_prefix, parsed.value, errors);
    }
}

const MigrationFileVersion = struct {
    from: i64,
    to: i64,
};

fn parseMigrationFileVersion(path: []const u8) ?MigrationFileVersion {
    const prefix = "migrations/";
    const suffix = ".json";
    if (!std.mem.startsWith(u8, path, prefix) or !std.mem.endsWith(u8, path, suffix)) return null;
    const stem = path[prefix.len .. path.len - suffix.len];
    const separator = std.mem.indexOf(u8, stem, "_to_") orelse return null;
    if (std.mem.indexOfScalar(u8, stem, '/') != null) return null;
    const from = std.fmt.parseInt(i64, stem[0..separator], 10) catch return null;
    const to = std.fmt.parseInt(i64, stem[separator + "_to_".len ..], 10) catch return null;
    if (from < 1 or to < 1) return null;
    return .{ .from = from, .to = to };
}

fn validateServerMigrationSteps(
    allocator: std.mem.Allocator,
    storage_prefix: []const u8,
    migration: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    const steps = migration.object.get("steps") orelse {
        try errors.append(allocator, "invalid_migration");
        return;
    };
    if (steps != .array) {
        try errors.append(allocator, "invalid_migration");
        return;
    }
    for (steps.array.items) |step| {
        if (step != .object) {
            try errors.append(allocator, "invalid_migration");
            continue;
        }
        const op = valueString(step.object.get("op")) orelse {
            try errors.append(allocator, "invalid_migration_op");
            continue;
        };
        if (!isAllowedMigrationOp(op)) {
            try errors.append(allocator, "invalid_migration_op");
        }
        try validateMigrationStringFieldPrefix(allocator, storage_prefix, step, "key", errors);
        try validateMigrationStringFieldPrefix(allocator, storage_prefix, step, "keyPattern", errors);
        if (std.mem.eql(u8, op, "renameKey") or std.mem.eql(u8, op, "moveStorageKey") or std.mem.eql(u8, op, "copyKey")) {
            try validateMigrationStringFieldPrefix(allocator, storage_prefix, step, "from", errors);
            try validateMigrationStringFieldPrefix(allocator, storage_prefix, step, "to", errors);
        }
    }
}

fn isAllowedMigrationOp(op: []const u8) bool {
    const ops = [_][]const u8{
        "renameKey",
        "setDefault",
        "deleteKey",
        "copyKey",
        "transformEnum",
        "moveStorageKey",
        "deleteStorageKey",
    };
    for (ops) |candidate| {
        if (std.mem.eql(u8, op, candidate)) return true;
    }
    return false;
}

fn validateMigrationStringFieldPrefix(
    allocator: std.mem.Allocator,
    storage_prefix: []const u8,
    step: std.json.Value,
    field: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    const value = valueString(step.object.get(field)) orelse return;
    if (!std.mem.startsWith(u8, value, storage_prefix)) try errors.append(allocator, "invalid_migration_prefix");
}

fn validateServerHtmlPolicy(
    allocator: std.mem.Allocator,
    html: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    try validateServerCsp(allocator, html, errors);
    try validateServerScriptTags(allocator, html, errors);
    if (htmlHasInlineEventHandler(html)) try errors.append(allocator, "forbidden_inline_handler");
    if (try htmlHasInlineStyle(allocator, html)) try errors.append(allocator, "forbidden_inline_style");
    if (containsIgnoreCase(html, "javascript:")) try errors.append(allocator, "forbidden_javascript_url");
    if (try htmlHasMetaRefresh(allocator, html)) try errors.append(allocator, "forbidden_meta_refresh");
    if (findOpeningTag(html, "base", 0) != null) try errors.append(allocator, "forbidden_base_href");
    if (try htmlHasForbiddenFormAction(allocator, html)) try errors.append(allocator, "forbidden_form_action");
    if (findOpeningTag(html, "iframe", 0) != null or
        findOpeningTag(html, "object", 0) != null or
        findOpeningTag(html, "embed", 0) != null or
        findOpeningTag(html, "applet", 0) != null)
    {
        try errors.append(allocator, "forbidden_embedded_context");
    }
    try validateServerStylesheetLinks(allocator, html, errors);
}

fn validateServerCsp(
    allocator: std.mem.Allocator,
    html: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    var index: usize = 0;
    while (findOpeningTag(html, "meta", index)) |start| {
        index = start + 1;
        const attrs = htmlOpeningTagAttrs(html, start, "meta") orelse continue;
        const http_equiv = try htmlAttrValueAlloc(allocator, attrs, "http-equiv");
        defer if (http_equiv) |actual| allocator.free(actual);
        if (http_equiv == null or !std.ascii.eqlIgnoreCase(http_equiv.?, "Content-Security-Policy")) continue;

        const content = try htmlAttrValueAlloc(allocator, attrs, "content");
        defer if (content) |actual| allocator.free(actual);
        const actual = content orelse continue;
        if (cspAllowsUnsafeInline(actual, "style-src")) {
            try errors.append(allocator, "forbidden_inline_style_csp");
        }
        if (cspAllowsUnsafeInline(actual, "script-src")) {
            try errors.append(allocator, "forbidden_inline_script_csp");
        }
    }
}

fn cspAllowsUnsafeInline(content: []const u8, directive: []const u8) bool {
    if (cspDirectiveHasToken(content, directive, "'unsafe-inline'")) return true;
    if (!std.ascii.eqlIgnoreCase(directive, "default-src") and !cspHasDirective(content, directive)) {
        return cspDirectiveHasToken(content, "default-src", "'unsafe-inline'");
    }
    return false;
}

fn cspHasDirective(content: []const u8, directive: []const u8) bool {
    var directives = std.mem.splitScalar(u8, content, ';');
    while (directives.next()) |raw| {
        const trimmed = std.mem.trim(u8, raw, " \t\r\n");
        if (cspDirectiveNameMatches(trimmed, directive)) return true;
    }
    return false;
}

fn cspDirectiveHasToken(content: []const u8, directive: []const u8, token: []const u8) bool {
    var directives = std.mem.splitScalar(u8, content, ';');
    while (directives.next()) |raw| {
        const trimmed = std.mem.trim(u8, raw, " \t\r\n");
        if (!cspDirectiveNameMatches(trimmed, directive)) continue;
        var tokens = std.mem.tokenizeAny(u8, trimmed[directive.len..], " \t\r\n");
        while (tokens.next()) |actual| {
            if (std.mem.eql(u8, actual, token)) return true;
        }
    }
    return false;
}

fn cspDirectiveNameMatches(directive_source: []const u8, directive: []const u8) bool {
    if (directive_source.len < directive.len) return false;
    if (!std.ascii.eqlIgnoreCase(directive_source[0..directive.len], directive)) return false;
    return directive_source.len == directive.len or htmlSpace(directive_source[directive.len]);
}

fn htmlHasForbiddenScriptTag(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "script", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "script") orelse return true;
        const src = try htmlAttrValueAlloc(allocator, attrs, "src");
        defer if (src) |actual| allocator.free(actual);
        if (src == null or !std.mem.eql(u8, src.?, "app.js")) return true;
        index = start + 1;
    }
    return false;
}

fn htmlHasRemoteScript(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "script", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "script") orelse return false;
        const src = try htmlAttrValueAlloc(allocator, attrs, "src");
        defer if (src) |actual| allocator.free(actual);
        if (src) |actual| {
            if (isHttpUrl(actual)) return true;
        }
        index = start + 1;
    }
    return false;
}

fn validateServerScriptTags(
    allocator: std.mem.Allocator,
    html: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    var script_count: usize = 0;
    var app_script_count: usize = 0;
    var index: usize = 0;
    while (findOpeningTag(html, "script", index)) |start| {
        script_count += 1;
        index = start + 1;
        const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse {
            try errors.append(allocator, "forbidden_inline_script");
            continue;
        };
        const attrs = htmlOpeningTagAttrs(html, start, "script") orelse {
            try errors.append(allocator, "forbidden_inline_script");
            continue;
        };
        const src = try htmlAttrValueAlloc(allocator, attrs, "src");
        defer if (src) |actual| allocator.free(actual);
        if (src) |actual| {
            if (isHttpUrl(actual)) {
                try errors.append(allocator, "forbidden_remote_script");
                continue;
            }
            if (!std.mem.eql(u8, actual, "app.js")) {
                try errors.append(allocator, "forbidden_app_script_src");
                continue;
            }
        } else {
            try errors.append(allocator, "forbidden_inline_script");
            continue;
        }

        app_script_count += 1;
        if (htmlAttrsContainDisallowedNames(attrs, &.{"src"})) {
            try errors.append(allocator, "forbidden_app_script_attribute");
        }
        const close_start = indexOfIgnoreCasePos(html, open_end + 1, "</script>") orelse html.len;
        if (std.mem.trim(u8, html[open_end + 1 .. close_start], " \t\r\n").len > 0) {
            try errors.append(allocator, "forbidden_inline_script");
        }
    }

    if (script_count == 0) {
        try errors.append(allocator, "missing_app_script");
    } else if (app_script_count != 1) {
        try errors.append(allocator, "invalid_app_script_count");
    }
}

fn htmlHasInlineStyle(allocator: std.mem.Allocator, html: []const u8) !bool {
    if (findOpeningTag(html, "style", 0) != null) return true;
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |start| {
        index = start + 1;
        const end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return false;
        const attrs = html[start + 1 .. end];
        const style = try htmlAttrValueAlloc(allocator, attrs, "style");
        defer if (style) |actual| allocator.free(actual);
        if (style != null) return true;
        index = end + 1;
    }
    return false;
}

fn htmlHasMetaRefresh(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "meta", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "meta") orelse return false;
        const http_equiv = try htmlAttrValueAlloc(allocator, attrs, "http-equiv");
        defer if (http_equiv) |actual| allocator.free(actual);
        if (http_equiv) |actual| {
            if (std.ascii.eqlIgnoreCase(actual, "refresh")) return true;
        }
        index = start + 1;
    }
    return false;
}

fn htmlHasForbiddenFormAction(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "form", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "form") orelse return false;
        const action = try htmlAttrValueAlloc(allocator, attrs, "action");
        defer if (action) |actual| allocator.free(actual);
        if (action) |actual| {
            if (!std.mem.eql(u8, actual, "#")) return true;
        }
        index = start + 1;
    }
    return false;
}

fn htmlHasRemoteStylesheet(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "link", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "link") orelse return false;
        if (try htmlRelIncludesStylesheet(allocator, attrs)) {
            const href = try htmlAttrValueAlloc(allocator, attrs, "href");
            defer if (href) |actual| allocator.free(actual);
            if (href) |actual| {
                if (isHttpUrl(actual)) return true;
            }
        }
        index = start + 1;
    }
    return false;
}

fn htmlHasForbiddenStylesheetHref(allocator: std.mem.Allocator, html: []const u8) !bool {
    var index: usize = 0;
    while (findOpeningTag(html, "link", index)) |start| {
        const attrs = htmlOpeningTagAttrs(html, start, "link") orelse return false;
        if (try htmlRelIncludesStylesheet(allocator, attrs)) {
            const href = try htmlAttrValueAlloc(allocator, attrs, "href");
            defer if (href) |actual| allocator.free(actual);
            if (href == null or !std.mem.eql(u8, href.?, "styles.css")) return true;
        }
        index = start + 1;
    }
    return false;
}

fn validateServerStylesheetLinks(
    allocator: std.mem.Allocator,
    html: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    var stylesheet_count: usize = 0;
    var index: usize = 0;
    while (findOpeningTag(html, "link", index)) |start| {
        index = start + 1;
        const attrs = htmlOpeningTagAttrs(html, start, "link") orelse continue;
        if (!try htmlRelIncludesStylesheet(allocator, attrs)) continue;

        const href = try htmlAttrValueAlloc(allocator, attrs, "href");
        defer if (href) |actual| allocator.free(actual);
        if (href) |actual| {
            if (isHttpUrl(actual)) {
                try errors.append(allocator, "forbidden_remote_stylesheet");
                continue;
            }
            if (!std.mem.eql(u8, actual, "styles.css")) {
                try errors.append(allocator, "forbidden_stylesheet_href");
                continue;
            }
        } else {
            try errors.append(allocator, "forbidden_stylesheet_href");
            continue;
        }

        stylesheet_count += 1;
        if (htmlAttrsContainDisallowedNames(attrs, &.{ "rel", "href" })) {
            try errors.append(allocator, "forbidden_stylesheet_attribute");
        }
    }

    if (stylesheet_count == 0) {
        try errors.append(allocator, "missing_stylesheet");
    } else if (stylesheet_count > 1) {
        try errors.append(allocator, "invalid_stylesheet_count");
    }
}

fn htmlRelIncludesStylesheet(allocator: std.mem.Allocator, attrs: []const u8) !bool {
    const rel = try htmlAttrValueAlloc(allocator, attrs, "rel");
    defer if (rel) |actual| allocator.free(actual);
    const actual = rel orelse return false;
    var tokens = std.mem.tokenizeAny(u8, actual, " \t\r\n");
    while (tokens.next()) |token| {
        if (std.ascii.eqlIgnoreCase(token, "stylesheet")) return true;
    }
    return false;
}

fn htmlAttrsContainDisallowedNames(attrs: []const u8, allowed: []const []const u8) bool {
    var cursor: usize = 0;
    while (cursor < attrs.len) {
        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor >= attrs.len or attrs[cursor] == '/') break;

        const name_start = cursor;
        while (cursor < attrs.len and htmlAttrNameChar(attrs[cursor])) : (cursor += 1) {}
        if (cursor == name_start) {
            cursor += 1;
            continue;
        }
        if (!htmlAttrNameAllowed(attrs[name_start..cursor], allowed)) return true;

        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor >= attrs.len or attrs[cursor] != '=') continue;
        cursor += 1;
        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor >= attrs.len) break;
        if (attrs[cursor] == '"' or attrs[cursor] == '\'') {
            const quote = attrs[cursor];
            cursor += 1;
            while (cursor < attrs.len and attrs[cursor] != quote) : (cursor += 1) {}
            if (cursor < attrs.len) cursor += 1;
        } else {
            while (cursor < attrs.len and !htmlSpace(attrs[cursor]) and attrs[cursor] != '>') : (cursor += 1) {}
        }
    }
    return false;
}

fn htmlAttrNameAllowed(name: []const u8, allowed: []const []const u8) bool {
    for (allowed) |candidate| {
        if (std.ascii.eqlIgnoreCase(name, candidate)) return true;
    }
    return false;
}

fn htmlAttrNameChar(char: u8) bool {
    return htmlNameChar(char) or char == '_' or char == ':';
}

fn htmlOpeningTagAttrs(html: []const u8, start: usize, tag: []const u8) ?[]const u8 {
    const open_end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return null;
    const attrs_start = start + 1 + tag.len;
    if (attrs_start > open_end) return null;
    return html[attrs_start..open_end];
}

fn htmlHasInlineEventHandler(html: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |start| {
        const end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return false;
        const attrs = html[start + 1 .. end];
        if (attrsHaveInlineEventHandler(attrs)) return true;
        index = end + 1;
    }
    return false;
}

fn attrsHaveInlineEventHandler(attrs: []const u8) bool {
    var index: usize = 0;
    while (index + 2 < attrs.len) : (index += 1) {
        if (std.ascii.toLower(attrs[index]) != 'o' or std.ascii.toLower(attrs[index + 1]) != 'n') continue;
        if (index > 0 and htmlNameChar(attrs[index - 1])) continue;
        var cursor = index + 2;
        while (cursor < attrs.len and std.ascii.isAlphabetic(attrs[cursor])) : (cursor += 1) {}
        if (cursor == index + 2) continue;
        while (cursor < attrs.len and htmlSpace(attrs[cursor])) : (cursor += 1) {}
        if (cursor < attrs.len and attrs[cursor] == '=') return true;
    }
    return false;
}

fn containsIgnoreCase(source: []const u8, needle: []const u8) bool {
    return indexOfIgnoreCasePos(source, 0, needle) != null;
}

fn indexOfIgnoreCasePos(source: []const u8, start_index: usize, needle: []const u8) ?usize {
    if (needle.len == 0) return start_index;
    if (source.len < needle.len or start_index > source.len - needle.len) return null;
    var index = start_index;
    while (index + needle.len <= source.len) : (index += 1) {
        if (std.ascii.eqlIgnoreCase(source[index .. index + needle.len], needle)) return index;
    }
    return null;
}

fn isHttpUrl(value: []const u8) bool {
    return startsWithIgnoreCase(value, "http://") or startsWithIgnoreCase(value, "https://");
}

fn validateServerCssPolicy(
    allocator: std.mem.Allocator,
    css: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    if (containsIgnoreCase(css, "@import")) try errors.append(allocator, "forbidden_css_import");
    if (containsIgnoreCase(css, "@font-face")) try errors.append(allocator, "forbidden_external_font");
    if (cssHasFixedPosition(css)) try errors.append(allocator, "forbidden_fixed_position");
    if (cssHasForbiddenUrl(css)) try errors.append(allocator, "forbidden_css_url");
}

fn cssHasFixedPosition(css: []const u8) bool {
    var index: usize = 0;
    while (indexOfIgnoreCasePos(css, index, "position")) |start| {
        var cursor = start + "position".len;
        while (cursor < css.len and htmlSpace(css[cursor])) : (cursor += 1) {}
        if (cursor >= css.len or css[cursor] != ':') {
            index = cursor;
            continue;
        }
        cursor += 1;
        while (cursor < css.len and htmlSpace(css[cursor])) : (cursor += 1) {}
        const end = cursor + "fixed".len;
        if (end <= css.len and std.ascii.eqlIgnoreCase(css[cursor..end], "fixed")) {
            if (end == css.len or !isCssIdentifierChar(css[end])) return true;
        }
        index = cursor + 1;
    }
    return false;
}

fn cssHasForbiddenUrl(css: []const u8) bool {
    var index: usize = 0;
    while (indexOfIgnoreCasePos(css, index, "url(")) |start| {
        var cursor = start + "url(".len;
        while (cursor < css.len and htmlSpace(css[cursor])) : (cursor += 1) {}
        if (cursor < css.len and (css[cursor] == '"' or css[cursor] == '\'')) cursor += 1;
        const value = css[cursor..];
        if (startsWithIgnoreCase(value, "http:") or
            startsWithIgnoreCase(value, "https:") or
            startsWithIgnoreCase(value, "data:") or
            startsWithIgnoreCase(value, "/"))
        {
            return true;
        }
        index = cursor + 1;
    }
    return false;
}

fn isCssIdentifierChar(char: u8) bool {
    return std.ascii.isAlphanumeric(char) or char == '-' or char == '_';
}

fn validateServerJsPolicy(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    js: []const u8,
    errors: *std.ArrayList([]const u8),
) !void {
    if (jsHasCall(js, "eval")) try errors.append(allocator, "forbidden_eval");
    if (jsHasNewFunction(js)) try errors.append(allocator, "forbidden_function_constructor");
    if (jsHasCall(js, "import")) try errors.append(allocator, "forbidden_dynamic_import");
    if (containsAny(js, &.{ "navigator.serviceWorker", "serviceWorker.register" })) try errors.append(allocator, "forbidden_service_worker");
    if (containsAny(js, &.{"trustedTypes.createPolicy"})) try errors.append(allocator, "forbidden_trusted_types_policy");
    if (jsHasCall(js, "fetch") or containsAny(js, &.{ "XMLHttpRequest", "WebSocket", "EventSource", "navigator.sendBeacon" })) try errors.append(allocator, "forbidden_network_api");
    if (containsAny(js, &.{ "localStorage", "sessionStorage", "indexedDB", "document.cookie", "cookieStore" })) try errors.append(allocator, "forbidden_storage_api");
    if (jsHasCall(js, "openDatabase") or jsHasCall(js, "executeSql") or containsAny(js, &.{ "SQLDatabase", "sqlite3" })) try errors.append(allocator, "forbidden_sql_api");
    if (containsAny(js, &.{ "webkit.messageHandlers", "chrome.webview", "Android.", "native.exec", "NativeAIPlatformBridge" })) try errors.append(allocator, "forbidden_native_bridge");
    if (containsAny(js, &.{ "window.parent", "window.top", "window.opener" })) try errors.append(allocator, "forbidden_parent_access");
    if (containsAny(js, &.{"shell.exec"})) try errors.append(allocator, "forbidden_bridge_method");
    if (hasRuntimeBridgeCallAppIdParam(js)) try errors.append(allocator, "forbidden_appid_param");
    if (hasUnknownRuntimeBridgeCall(js)) try errors.append(allocator, "forbidden_bridge_method");
    if (hasRuntimeBridgeCallMissingPermission(manifest, js)) try errors.append(allocator, "missing_permission");
}

fn jsHasCall(source: []const u8, name: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, source, index, name)) |start| {
        index = start + name.len;
        if (!isJavaScriptStandaloneToken(source, start, name.len)) continue;
        var cursor = start + name.len;
        while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
        if (cursor < source.len and source[cursor] == '(') return true;
    }
    return false;
}

fn jsHasNewFunction(source: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, source, index, "new")) |start| {
        index = start + "new".len;
        if (!isJavaScriptStandaloneToken(source, start, "new".len)) continue;
        var cursor = start + "new".len;
        if (cursor >= source.len or !isJsonWhitespace(source[cursor])) continue;
        while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
        const function_end = cursor + "Function".len;
        if (function_end > source.len or !std.mem.eql(u8, source[cursor..function_end], "Function")) continue;
        if (!isJavaScriptStandaloneToken(source, cursor, "Function".len)) continue;
        cursor = function_end;
        while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
        if (cursor < source.len and source[cursor] == '(') return true;
    }
    return false;
}

fn isJavaScriptStandaloneToken(source: []const u8, start: usize, len: usize) bool {
    if (start > 0) {
        const previous = source[start - 1];
        if (isJavaScriptIdentifierChar(previous) or previous == '.') return false;
    }
    const end = start + len;
    if (end < source.len and isJavaScriptIdentifierChar(source[end])) return false;
    return true;
}

fn validateServerNetworkPolicy(
    allocator: std.mem.Allocator,
    network_policy: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (network_policy != .object) {
        try errors.append(allocator, "invalid_network_policy");
        return;
    }
    var policy_iterator = network_policy.object.iterator();
    while (policy_iterator.next()) |entry| {
        if (!isAllowedNetworkPolicyKey(entry.key_ptr.*)) {
            try errors.append(allocator, "invalid_network_policy");
        }
    }
    for ([_][]const u8{ "denyPrivateNetwork", "allowCredentials" }) |key| {
        if (network_policy.object.get(key)) |value| {
            if (value != .bool) try errors.append(allocator, "invalid_network_policy");
        }
    }
    if (network_policy.object.get("allowCredentials")) |value| {
        if (value == .bool and value.bool) try errors.append(allocator, "invalid_network_policy");
    }
    const allow = network_policy.object.get("allow") orelse {
        try errors.append(allocator, "invalid_network_policy");
        return;
    };
    if (allow != .array) {
        try errors.append(allocator, "invalid_network_policy");
        return;
    }
    for (allow.array.items) |entry| {
        if (entry != .object) {
            try errors.append(allocator, "invalid_network_policy");
            continue;
        }
        var entry_iterator = entry.object.iterator();
        while (entry_iterator.next()) |field| {
            if (!isAllowedNetworkPolicyEntryKey(field.key_ptr.*)) {
                try errors.append(allocator, "invalid_network_policy");
            }
        }
        const origin = valueString(entry.object.get("origin")) orelse {
            try errors.append(allocator, "invalid_network_origin");
            continue;
        };
        if (!isValidHttpsOrigin(origin)) try errors.append(allocator, "invalid_network_origin");
        const methods = entry.object.get("methods") orelse {
            try errors.append(allocator, "invalid_network_methods");
            continue;
        };
        if (methods != .array) {
            try errors.append(allocator, "invalid_network_methods");
            continue;
        }
        if (methods.array.items.len == 0) {
            try errors.append(allocator, "invalid_network_methods");
            continue;
        }
        for (methods.array.items, 0..) |method, index| {
            const method_name = valueString(method) orelse {
                try errors.append(allocator, "invalid_network_methods");
                continue;
            };
            if (!isAllowedNetworkMethod(method_name) or stringValueAppearsBefore(methods.array.items, index, method_name)) {
                try errors.append(allocator, "invalid_network_methods");
            }
        }
        if (entry.object.get("pathPrefix")) |path_prefix| {
            if (path_prefix != .string) try errors.append(allocator, "invalid_network_policy");
        }
        if (entry.object.get("allowedHeaders")) |headers| {
            if (headers != .array) {
                try errors.append(allocator, "invalid_network_policy");
            } else {
                for (headers.array.items, 0..) |header, index| {
                    const header_name = valueString(header) orelse {
                        try errors.append(allocator, "invalid_network_policy");
                        continue;
                    };
                    if (isCredentialHeader(header_name) or stringValueAppearsBefore(headers.array.items, index, header_name)) {
                        try errors.append(allocator, "invalid_network_policy");
                    }
                }
            }
        }
        for ([_][]const u8{ "maxRequestBytes", "maxResponseBytes" }) |key| {
            if (entry.object.get(key)) |limit| {
                if (limit != .integer or limit.integer < 0) try errors.append(allocator, "invalid_network_policy");
            }
        }
        if (entry.object.get("timeoutMs")) |timeout| {
            if (timeout != .integer or timeout.integer < 1 or timeout.integer > 120000) {
                try errors.append(allocator, "invalid_network_policy");
            }
        }
    }
}

fn isAllowedNetworkPolicyKey(key: []const u8) bool {
    const keys = [_][]const u8{ "allow", "denyPrivateNetwork", "allowCredentials" };
    for (keys) |candidate| {
        if (std.mem.eql(u8, key, candidate)) return true;
    }
    return false;
}

fn isAllowedNetworkPolicyEntryKey(key: []const u8) bool {
    const keys = [_][]const u8{
        "origin",
        "methods",
        "pathPrefix",
        "allowedHeaders",
        "maxRequestBytes",
        "maxResponseBytes",
        "timeoutMs",
    };
    for (keys) |candidate| {
        if (std.mem.eql(u8, key, candidate)) return true;
    }
    return false;
}

fn isAllowedNetworkMethod(method: []const u8) bool {
    const methods = [_][]const u8{ "GET", "POST", "PUT", "PATCH", "DELETE" };
    for (methods) |candidate| {
        if (std.mem.eql(u8, method, candidate)) return true;
    }
    return false;
}

fn stringValueAppearsBefore(items: []std.json.Value, index: usize, value: []const u8) bool {
    for (items[0..index]) |candidate| {
        if (valueString(candidate)) |candidate_value| {
            if (std.mem.eql(u8, candidate_value, value)) return true;
        }
    }
    return false;
}

fn isValidHttpsOrigin(origin: []const u8) bool {
    const prefix = "https://";
    if (!std.mem.startsWith(u8, origin, prefix)) return false;
    const host = origin[prefix.len..];
    if (host.len == 0) return false;
    for (host) |char| {
        if (char == '/' or char == '?' or char == '#' or char == ' ' or char == '\t' or char == '\n' or char == '\r') {
            return false;
        }
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
    const levels = [_][]const u8{ "info", "success", "warning", "error" };
    for (levels) |candidate| {
        if (std.mem.eql(u8, level, candidate)) return true;
    }
    return false;
}

fn toastLevelDetailsJsonAlloc(allocator: std.mem.Allocator, level: []const u8) ![]u8 {
    const escaped_level = try escapeJsonString(allocator, level);
    defer allocator.free(escaped_level);
    return std.fmt.allocPrint(allocator, "{{\"level\":\"{s}\"}}", .{escaped_level});
}

fn upperAsciiAlloc(allocator: std.mem.Allocator, value: []const u8) ![]u8 {
    const out = try allocator.dupe(u8, value);
    for (out) |*char| {
        char.* = std.ascii.toUpper(char.*);
    }
    return out;
}

fn dialogTypeForBridgeMethod(method: []const u8) ?[]const u8 {
    if (std.mem.eql(u8, method, "dialog.openFile")) return "openFile";
    if (std.mem.eql(u8, method, "dialog.saveFile")) return "saveFile";
    return null;
}

fn normalizeDialogType(dialog_type: []const u8) ?[]const u8 {
    if (std.mem.eql(u8, dialog_type, "openFile") or std.mem.eql(u8, dialog_type, "dialog.openFile")) return "openFile";
    if (std.mem.eql(u8, dialog_type, "saveFile") or std.mem.eql(u8, dialog_type, "dialog.saveFile")) return "saveFile";
    return null;
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

fn validateServerPackageFileList(
    allocator: std.mem.Allocator,
    files: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (files != .array) {
        try errors.append(allocator, "invalid_files");
        return;
    }
    if (files.array.items.len > max_package_files) try errors.append(allocator, "resource_budget_exceeded");
    var migration_file_count: usize = 0;
    for (files.array.items) |file| {
        if (file != .object) {
            try errors.append(allocator, "invalid_files");
            continue;
        }
        const file_path = valueString(file.object.get("path")) orelse {
            try errors.append(allocator, "invalid_files");
            continue;
        };
        if (isPlatformGeneratedPackagePath(file_path)) {
            try errors.append(allocator, "platform_generated_artifact");
            continue;
        }
        if (std.mem.startsWith(u8, file_path, "assets/") or !isAllowedServerPackagePath(file_path)) {
            try errors.append(allocator, "unexpected_package_path");
        }
        if (std.mem.startsWith(u8, file_path, "migrations/")) migration_file_count += 1;
    }
    if (migration_file_count > max_migration_files) try errors.append(allocator, "resource_budget_exceeded");
}

fn isAllowedServerPackagePath(file_path: []const u8) bool {
    const allowed_files = [_][]const u8{
        "manifest.json",
        "index.html",
        "styles.css",
        "app.js",
        "smoke-tests.json",
        "README.md",
    };
    for (allowed_files) |allowed| {
        if (std.mem.eql(u8, file_path, allowed)) return true;
    }
    return std.mem.startsWith(u8, file_path, "migrations/");
}

fn isPlatformGeneratedPackagePath(file_path: []const u8) bool {
    const generated_files = [_][]const u8{
        "signature.json",
        "install-report.json",
        "content-hashes.json",
    };
    for (generated_files) |generated| {
        if (std.mem.eql(u8, file_path, generated)) return true;
    }
    return false;
}

fn containsAny(source: []const u8, needles: []const []const u8) bool {
    for (needles) |needle| {
        if (std.mem.indexOf(u8, source, needle) != null) return true;
    }
    return false;
}

const RuntimeBridgeCall = struct {
    method: []const u8,
    next_index: usize,
    malformed: bool = false,
};

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
    const methods = [_][]const u8{};
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
    while (nextRuntimeBridgeCall(source, &index)) |call| {
        if (call.malformed) return true;
        if (!isAllowedRuntimeBridgeMethod(call.method)) return true;
    }
    return false;
}

fn hasRuntimeBridgeCallMissingPermission(manifest: std.json.Value, source: []const u8) bool {
    var index: usize = 0;
    while (nextRuntimeBridgeCall(source, &index)) |call| {
        if (call.malformed or !isAllowedRuntimeBridgeMethod(call.method)) continue;
        const permission = permissionForBridgeMethod(call.method) orelse continue;
        if (!manifestPermissionsContain(manifest, permission)) return true;
    }
    return false;
}

fn hasRuntimeBridgeCallAppIdParam(source: []const u8) bool {
    var index: usize = 0;
    while (nextRuntimeBridgeCall(source, &index)) |call| {
        if (call.malformed) continue;
        if (runtimeBridgeCallHasAppIdParam(source, call.next_index)) return true;
    }
    return false;
}

fn runtimeBridgeCallHasAppIdParam(source: []const u8, after_method: usize) bool {
    var cursor = after_method;
    while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
    if (cursor >= source.len or source[cursor] != ',') return false;
    cursor += 1;
    while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
    if (cursor >= source.len or source[cursor] != '{') return false;

    const close = std.mem.indexOfScalarPos(u8, source, cursor, ')') orelse source.len;
    const params = source[cursor..close];
    return objectLiteralHasPropertyKey(params, "appId");
}

fn objectLiteralHasPropertyKey(source: []const u8, key: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, source, index, key)) |start| {
        index = start + key.len;
        var cursor = start + key.len;
        if (start > 0 and (source[start - 1] == '"' or source[start - 1] == '\'')) {
            const quote = source[start - 1];
            if (cursor >= source.len or source[cursor] != quote) continue;
            cursor += 1;
            while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
            if (cursor < source.len and source[cursor] == ':') return true;
            continue;
        }
        while (cursor < source.len and isJsonWhitespace(source[cursor])) : (cursor += 1) {}
        if (cursor >= source.len or source[cursor] != ':') continue;
        if (isJavaScriptStandaloneToken(source, start, key.len)) return true;
    }
    return false;
}

fn nextRuntimeBridgeCall(source: []const u8, index: *usize) ?RuntimeBridgeCall {
    const app_pattern = "AppRuntime.call";
    const bare_pattern = "call";

    while (index.* < source.len) {
        const app_call = std.mem.indexOfPos(u8, source, index.*, app_pattern);
        const bare_call = std.mem.indexOfPos(u8, source, index.*, bare_pattern);
        if (app_call == null and bare_call == null) return null;

        var start: usize = undefined;
        var after_pattern: usize = undefined;
        var bare = false;
        if (app_call) |app_start| {
            if (bare_call) |bare_start| {
                if (app_start <= bare_start) {
                    start = app_start;
                    after_pattern = app_start + app_pattern.len;
                } else {
                    start = bare_start;
                    after_pattern = bare_start + bare_pattern.len;
                    bare = true;
                }
            } else {
                start = app_start;
                after_pattern = app_start + app_pattern.len;
            }
        } else {
            start = bare_call.?;
            after_pattern = start + bare_pattern.len;
            bare = true;
        }

        index.* = after_pattern;
        if (bare and !isStandaloneCallToken(source, start, bare_pattern.len)) continue;

        const parsed = parseRuntimeBridgeCallAfter(source, after_pattern) orelse continue;
        index.* = parsed.next_index;
        return parsed;
    }
    return null;
}

fn parseRuntimeBridgeCallAfter(source: []const u8, search_after: usize) ?RuntimeBridgeCall {
    const open = std.mem.indexOfScalarPos(u8, source, search_after, '(') orelse return null;
    var cursor = open + 1;
    while (cursor < source.len and isJsonWhitespace(source[cursor])) {
        cursor += 1;
    }
    if (cursor >= source.len or (source[cursor] != '"' and source[cursor] != '\'')) return null;
    const quote = source[cursor];
    const method_start = cursor + 1;
    const method_end = std.mem.indexOfScalarPos(u8, source, method_start, quote) orelse
        return .{ .method = "", .next_index = source.len, .malformed = true };
    return .{ .method = source[method_start..method_end], .next_index = method_end + 1 };
}

fn isStandaloneCallToken(source: []const u8, start: usize, len: usize) bool {
    if (start > 0) {
        const previous = source[start - 1];
        if (isJavaScriptIdentifierChar(previous) or previous == '.') return false;
    }
    const end = start + len;
    if (end < source.len and isJavaScriptIdentifierChar(source[end])) return false;
    return true;
}

fn isJavaScriptIdentifierChar(char: u8) bool {
    return (char >= 'a' and char <= 'z') or
        (char >= 'A' and char <= 'Z') or
        (char >= '0' and char <= '9') or
        char == '_' or
        char == '$';
}

fn isJsonWhitespace(char: u8) bool {
    return char == ' ' or char == '\t' or char == '\n' or char == '\r';
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

test "runtime static snapshot helpers summarize installed app HTML" {
    const html =
        \\<!doctype html>
        \\<html>
        \\  <head><title>Notes Lite</title></head>
        \\  <body>
        \\    <main data-testid="notes-shell">
        \\      <h1>Notes Lite</h1>
        \\      <label>Title <input id="note-title" data-testid="note-title-input"></label>
        \\      <button data-testid="new-note-button">Create note</button>
        \\    </main>
        \\  </body>
        \\</html>
    ;

    const title = try htmlTitleOrFallbackAlloc(std.testing.allocator, html, "fallback");
    defer std.testing.allocator.free(title);
    try std.testing.expectEqualStrings("Notes Lite", title);

    const text = try htmlTextAlloc(std.testing.allocator, html);
    defer std.testing.allocator.free(text);
    try std.testing.expect(std.mem.indexOf(u8, text, "Create note") != null);

    const test_ids = try htmlDataTestIdsJsonAlloc(std.testing.allocator, html);
    defer std.testing.allocator.free(test_ids);
    try std.testing.expect(std.mem.indexOf(u8, test_ids, "\"new-note-button\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, test_ids, "\"notes-shell\"") != null);

    const accessibility = try htmlAccessibilityTreeJsonAlloc(std.testing.allocator, "notes-lite", html, title);
    defer std.testing.allocator.free(accessibility);
    try std.testing.expect(std.mem.indexOf(u8, accessibility, "\"role\":\"main\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, accessibility, "\"level\":1") != null);
    try std.testing.expect(std.mem.indexOf(u8, accessibility, "\"name\":\"Title\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, accessibility, "\"name\":\"Create note\"") != null);

    var query_args = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, "{\"testId\":\"new-note-button\"}", .{});
    defer query_args.deinit();
    const match = (try runtimeFirstMatchJsonAlloc(std.testing.allocator, html, query_args.value)).?;
    defer std.testing.allocator.free(match);
    try std.testing.expect(std.mem.indexOf(u8, match, "\"kind\":\"testId\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, match, "\"tag\":\"button\"") != null);

    var text_args = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, "{\"text\":\"Create note\"}", .{});
    defer text_args.deinit();
    const text_match = (try runtimeFirstMatchJsonAlloc(std.testing.allocator, html, text_args.value)).?;
    defer std.testing.allocator.free(text_match);
    try std.testing.expect(std.mem.indexOf(u8, text_match, "\"kind\":\"text\"") != null);
}

test "control auth tracker bans repeated failures and clears after expiry" {
    var tracker = ControlAuthTracker{};
    var key = ControlClientKey{ .family = std.posix.AF.INET, .len = 4 };
    key.bytes[0] = 127;
    key.bytes[3] = 1;

    try std.testing.expectError(error.ControlAuthRequired, authorizeControlTokenValue(&tracker, key, "expected-token", null, 0));
    try std.testing.expectError(error.ControlAuthRequired, authorizeControlTokenValue(&tracker, key, "expected-token", "wrong-token", 1));
    try std.testing.expectError(error.ControlConnectionBanned, authorizeControlTokenValue(&tracker, key, "expected-token", "wrong-token", 2));
    try std.testing.expectError(error.ControlConnectionBanned, authorizeControlTokenValue(&tracker, key, "expected-token", "expected-token", 3));
    try std.testing.expectEqual(@as(i64, 60), tracker.retryAfterSeconds(key, 2));

    try authorizeControlTokenValue(&tracker, key, "expected-token", "expected-token", control_auth_ban_ms + 3);
    try std.testing.expectEqual(@as(i64, 0), tracker.retryAfterSeconds(key, control_auth_ban_ms + 3));
}

test "generated control tokens are url-safe unpadded base64" {
    const token = try generateControlToken(std.testing.allocator);
    defer std.testing.allocator.free(token);

    try std.testing.expectEqual(@as(usize, 43), token.len);
    for (token) |char| {
        try std.testing.expect((char >= 'A' and char <= 'Z') or
            (char >= 'a' and char <= 'z') or
            (char >= '0' and char <= '9') or
            char == '-' or
            char == '_');
    }
}

test "control token writer creates private token file" {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const token_file = try std.fs.path.join(std.testing.allocator, &.{ ".zig-cache", "tmp", tmp.sub_path[0..], "nested", "control.token" });
    defer std.testing.allocator.free(token_file);

    try writeControlTokenFile(token_file, "test-token");

    const contents = try std.fs.cwd().readFileAlloc(std.testing.allocator, token_file, 128);
    defer std.testing.allocator.free(contents);
    try std.testing.expectEqualStrings("test-token\n", contents);

    if (builtin.os.tag != .windows) {
        const file = try std.fs.cwd().openFile(token_file, .{});
        defer file.close();
        const stat = try file.stat();
        try std.testing.expectEqual(@as(std.fs.File.Mode, 0o600), stat.mode & 0o777);
    }
}

test "production mode classifies dev control paths and startup flags" {
    const dev_paths = [_][]const u8{
        "/control/command",
        "/control/db/snapshot",
        "/db/snapshot",
        "/db/app-storage",
        "/webapps/validate",
        "/webapps/install",
        "/packages/validate",
        "/control/packages/sign",
        "/apps/notes-lite/rollback",
    };
    for (dev_paths) |path| {
        try std.testing.expect(isDevControlPath(path));
    }

    const runtime_paths = [_][]const u8{
        "/health",
        "/core/step",
        "/bridge",
        "/webapps/examples",
        "/webapps/examples.json",
        "/webapps/examples/notes-lite/index.html",
    };
    for (runtime_paths) |path| {
        try std.testing.expect(!isDevControlPath(path));
    }

    const forbidden_flags = [_][]const u8{
        "--control-plane-port",
        "--control-plane-port=9988",
        "--allow-runtime-mismatch",
        "--allow-runtime-mismatch=true",
        "--allow-unsigned-dev",
        "--allow-unsigned-dev=true",
        "--token-file",
        "--token-file=control.token",
    };
    for (forbidden_flags) |flag| {
        try std.testing.expect(isForbiddenProductionFlag(flag));
    }

    const allowed_flags = [_][]const u8{
        "--port",
        "--port=8080",
        "--control-plane-portish",
        "--allow-runtime-mismatchish",
        "--allow-unsigned-devish",
        "--token-fileish",
    };
    for (allowed_flags) |flag| {
        try std.testing.expect(!isForbiddenProductionFlag(flag));
    }
}

test "control storage helpers enforce app storage prefix" {
    try std.testing.expect(try storageKeyHasAppPrefix(std.testing.allocator, "notes-lite", "notes-lite:notes"));
    try std.testing.expect(!(try storageKeyHasAppPrefix(std.testing.allocator, "notes-lite", "other:notes")));
}

test "resource budget bridge errors include repair details" {
    const details = try resourceBudgetDetailsJsonAlloc(std.testing.allocator, .{
        .message = "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute",
        .app_id = "notes-lite",
        .budget = "maxBridgeCallsPerMinute",
        .current = 601,
        .max = 600,
    });
    defer std.testing.allocator.free(details);
    try std.testing.expectEqualStrings("{\"appId\":\"notes-lite\",\"budget\":\"maxBridgeCallsPerMinute\",\"current\":601,\"max\":600,\"limit\":600}", details);

    const response = try bridgeErrorResponseJsonWithDetailsAlloc(
        std.testing.allocator,
        "req_budget",
        "resource_budget_exceeded",
        "maxBridgeCallsPerMinute exceeded",
        details,
    );
    defer std.testing.allocator.free(response);
    try std.testing.expect(std.mem.indexOf(u8, response, "\"details\":{\"budget\":\"maxBridgeCallsPerMinute\",\"current\":601,\"max\":600") != null);
}

test "server package validation rejects direct native bridge globals" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "native-bridge-test",
        \\    "name": "Native Bridge Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "native-bridge-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Native bridge test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "NativeAIPlatformBridge.postMessage({});"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_native_bridge\"") != null);
}

test "server JS policy detects bridge appId params" {
    try std.testing.expect(hasRuntimeBridgeCallAppIdParam(
        "AppRuntime.call(\"storage.get\", { appId: \"other-app\", key: \"notes-lite:notes\" });",
    ));
    try std.testing.expect(hasRuntimeBridgeCallAppIdParam(
        "call('storage.get', { 'appId': 'other-app', key: 'notes-lite:notes' });",
    ));
    try std.testing.expect(!hasRuntimeBridgeCallAppIdParam(
        "AppRuntime.call(\"storage.get\", { key: \"notes-lite:notes\" });",
    ));
}

test "server JS policy rejects cookie store API" {
    var manifest_json = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, "{\"permissions\":[]}", .{});
    defer manifest_json.deinit();

    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(std.testing.allocator);
    try validateServerJsPolicy(std.testing.allocator, manifest_json.value, "cookieStore.get('session');", &errors);

    var found = false;
    for (errors.items) |error_name| {
        if (std.mem.eql(u8, error_name, "forbidden_storage_api")) found = true;
    }
    try std.testing.expect(found);
}

test "server JS policy rejects sendBeacon network API" {
    var manifest_json = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, "{\"permissions\":[]}", .{});
    defer manifest_json.deinit();

    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(std.testing.allocator);
    try validateServerJsPolicy(std.testing.allocator, manifest_json.value, "navigator.sendBeacon('https://example.com/collect', '{}');", &errors);

    var found = false;
    for (errors.items) |error_name| {
        if (std.mem.eql(u8, error_name, "forbidden_network_api")) found = true;
    }
    try std.testing.expect(found);
}

test "server package validation rejects bridge calls without manifest permission" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "missing-permission-test",
        \\    "name": "Missing Permission Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "missing-permission-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Missing permission test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "AppRuntime.call(\"storage.get\", { key: \"missing-permission-test:notes\" });"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"missing_permission\"") != null);
}

test "server package validation requires content ratings for bundled manifests" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "bundled-rating-test",
        \\    "name": "Bundled Rating Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "bundled-rating-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []},
        \\    "trust": {"level": "bundled", "requiresUserApproval": false}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Bundled rating test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"missing_content_rating\"") != null);
}

test "server package validation rejects invalid content rating age bands" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "invalid-rating-test",
        \\    "name": "Invalid Rating Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "invalid-rating-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []},
        \\    "contentRating": {"scheme": "app-store", "label": "9+", "minimumAge": 4, "descriptors": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Invalid rating test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"invalid_content_rating\"") != null);
}

test "server package validation rejects package files over resource budget" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "budget-test",
        \\    "name": "Budget Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "budget-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 8
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Budget test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"resource_budget_exceeded\"") != null);
}

test "server package validation enforces hard package and migration file caps" {
    var files_json: std.io.Writer.Allocating = .init(std.testing.allocator);
    errdefer files_json.deinit();
    try files_json.writer.writeAll("[");
    for (0..(max_package_files + 1)) |index| {
        if (index > 0) try files_json.writer.writeAll(",");
        try files_json.writer.print("{{\"path\":\"app.js\",\"content\":\"{d}\"}}", .{index});
    }
    try files_json.writer.writeAll("]");
    const files_raw = try files_json.toOwnedSlice();
    defer std.testing.allocator.free(files_raw);
    var files = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, files_raw, .{});
    defer files.deinit();

    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(std.testing.allocator);
    try validateServerPackageFileList(std.testing.allocator, files.value, &errors);
    var found_package_cap = false;
    for (errors.items) |error_name| {
        if (std.mem.eql(u8, error_name, "resource_budget_exceeded")) found_package_cap = true;
    }
    try std.testing.expect(found_package_cap);

    var migrations_json: std.io.Writer.Allocating = .init(std.testing.allocator);
    errdefer migrations_json.deinit();
    try migrations_json.writer.writeAll("[");
    for (0..(max_migration_files + 1)) |index| {
        if (index > 0) try migrations_json.writer.writeAll(",");
        try migrations_json.writer.print("{{\"path\":\"migrations/{d}_to_{d}.json\",\"content\":\"{{}}\"}}", .{ index + 1, index + 2 });
    }
    try migrations_json.writer.writeAll("]");
    const migrations_raw = try migrations_json.toOwnedSlice();
    defer std.testing.allocator.free(migrations_raw);
    var migrations = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, migrations_raw, .{});
    defer migrations.deinit();

    var migration_errors: std.ArrayList([]const u8) = .empty;
    defer migration_errors.deinit(std.testing.allocator);
    try validateServerPackageFileList(std.testing.allocator, migrations.value, &migration_errors);
    var found_migration_cap = false;
    for (migration_errors.items) |error_name| {
        if (std.mem.eql(u8, error_name, "resource_budget_exceeded")) found_migration_cap = true;
    }
    try std.testing.expect(found_migration_cap);
}

test "server package validation requires consecutive migration files" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "migration-test",
        \\    "name": "Migration Test",
        \\    "version": "0.2.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "migration-test:",
        \\    "dataVersion": 2,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Migration test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"migration_missing\"") != null);
}

test "server package validation rejects migration keys outside storage prefix" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "migration-prefix-test",
        \\    "name": "Migration Prefix Test",
        \\    "version": "0.2.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "migration-prefix-test:",
        \\    "dataVersion": 2,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Migration prefix test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"},
        \\    {"path": "migrations/1_to_2.json", "content": "{\"appId\":\"migration-prefix-test\",\"fromDataVersion\":1,\"toDataVersion\":2,\"steps\":[{\"op\":\"deleteKey\",\"key\":\"other:leak\"}]}"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"invalid_migration_prefix\"") != null);
}

test "server package validation rejects inline script tags with attributes" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "html-policy-test",
        \\    "name": "HTML Policy Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "html-policy-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>HTML policy test</main><script type=\"module\">alert(1)</script>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_inline_script\"") != null);
}

test "server package validation rejects unexpected stylesheet hrefs" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "stylesheet-policy-test",
        \\    "name": "Stylesheet Policy Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "stylesheet-policy-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<link rel=\"stylesheet\" href=\"theme.css\"><main>Stylesheet policy test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_stylesheet_href\"") != null);
}

test "server package validation requires a plain styles.css link" {
    const missing_report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "missing-stylesheet-test",
        \\    "name": "Missing Stylesheet Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "missing-stylesheet-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>Missing stylesheet test</main><script src=\"app.js\"></script>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(missing_report);

    try std.testing.expect(std.mem.indexOf(u8, missing_report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, missing_report, "\"missing_stylesheet\"") != null);

    const non_plain_report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "non-plain-stylesheet-test",
        \\    "name": "Non Plain Stylesheet Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "non-plain-stylesheet-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<link rel=\"stylesheet\" href=\"styles.css\" media=\"print\"><link rel=\"stylesheet\" href=\"styles.css\"><main>Non plain stylesheet test</main><script src=\"app.js\"></script>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(non_plain_report);

    try std.testing.expect(std.mem.indexOf(u8, non_plain_report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, non_plain_report, "\"forbidden_stylesheet_attribute\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, non_plain_report, "\"invalid_stylesheet_count\"") != null);
}

test "server package validation rejects css policy variants" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "css-policy-test",
        \\    "name": "CSS Policy Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "css-policy-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>CSS policy test</main>"},
        \\    {"path": "styles.css", "content": ".escape { POSITION : fixed; background: URL( 'https://cdn.example.test/pixel.png'); }"},
        \\    {"path": "app.js", "content": "const value = 1;"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_fixed_position\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_css_url\"") != null);
}

test "server package validation rejects js policy variants" {
    const report = try validateWebappPackage(std.testing.allocator,
        \\{
        \\  "manifest": {
        \\    "id": "js-policy-test",
        \\    "name": "JS Policy Test",
        \\    "version": "0.1.0",
        \\    "runtimeVersion": "0.1.0",
        \\    "entry": "index.html",
        \\    "description": "Validator regression fixture.",
        \\    "permissions": [],
        \\    "storagePrefix": "js-policy-test:",
        \\    "dataVersion": 1,
        \\    "capabilities": {"required": [], "optional": []},
        \\    "resourceBudget": {
        \\      "maxDomNodes": 2000,
        \\      "maxStorageBytes": 5242880,
        \\      "maxBridgeCallsPerMinute": 600,
        \\      "maxNetworkRequestsPerMinute": 60,
        \\      "maxTimers": 64,
        \\      "maxLogLinesPerMinute": 120,
        \\      "maxPackageBytes": 1048576,
        \\      "maxFileBytes": 524288
        \\    },
        \\    "networkPolicy": {"allow": []}
        \\  },
        \\  "files": [
        \\    {"path": "manifest.json", "content": "{}"},
        \\    {"path": "index.html", "content": "<main>JS policy test</main>"},
        \\    {"path": "styles.css", "content": ""},
        \\    {"path": "app.js", "content": "const make = new   Function(\"return 1\"); import (\"./module.js\"); fetch (\"https://example.test\"); const db = openDatabase(\"app\", \"1\", \"app\", 1024); db.transaction(tx => tx.executeSql(\"select 1\"));"}
        \\  ]
        \\}
    );
    defer std.testing.allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_function_constructor\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_dynamic_import\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_network_api\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "\"forbidden_sql_api\"") != null);
}

test "notification toast levels follow runtime spec" {
    try std.testing.expect(isToastLevel("info"));
    try std.testing.expect(isToastLevel("success"));
    try std.testing.expect(isToastLevel("warning"));
    try std.testing.expect(isToastLevel("error"));
    try std.testing.expect(!isToastLevel("warn"));
}

test "network policy helper denies private network hosts by default" {
    var default_policy = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "allow": []
        \\}
    ,
        .{},
    );
    defer default_policy.deinit();
    try std.testing.expect(networkPolicyDeniesPrivateNetwork(default_policy.value));

    var disabled_policy = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "allow": [],
        \\  "denyPrivateNetwork": false
        \\}
    ,
        .{},
    );
    defer disabled_policy.deinit();
    try std.testing.expect(!networkPolicyDeniesPrivateNetwork(disabled_policy.value));

    const loopback = try parseNetworkUrlAlloc(std.testing.allocator, "https://127.0.0.1/status");
    defer freeUrlParts(std.testing.allocator, loopback);
    try std.testing.expect(std.mem.eql(u8, loopback.origin, "https://127.0.0.1"));
    try std.testing.expect(std.mem.eql(u8, loopback.host, "127.0.0.1"));
    try std.testing.expect(isPrivateNetworkHost(loopback.host));

    const ipv6 = try parseNetworkUrlAlloc(std.testing.allocator, "https://[fd00::1]/status");
    defer freeUrlParts(std.testing.allocator, ipv6);
    try std.testing.expect(std.mem.eql(u8, ipv6.host, "fd00::1"));
    try std.testing.expect(isPrivateNetworkHost(ipv6.host));

    const mapped_loopback = try parseNetworkUrlAlloc(std.testing.allocator, "https://[::ffff:7f00:1]/status");
    defer freeUrlParts(std.testing.allocator, mapped_loopback);
    try std.testing.expect(std.mem.eql(u8, mapped_loopback.host, "::ffff:7f00:1"));
    try std.testing.expect(isPrivateNetworkHost(mapped_loopback.host));

    const public = try parseNetworkUrlAlloc(std.testing.allocator, "https://api.example.com/status");
    defer freeUrlParts(std.testing.allocator, public);
    try std.testing.expect(!isPrivateNetworkHost(public.host));
}

test "server network policy validation rejects credential opt-in" {
    var policy_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "allow": [],
        \\  "allowCredentials": true
        \\}
    ,
        .{},
    );
    defer policy_json.deinit();

    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(std.testing.allocator);
    try validateServerNetworkPolicy(std.testing.allocator, policy_json.value, &errors);
    try std.testing.expect(errors.items.len > 0);
    var found = false;
    for (errors.items) |error_name| {
        if (std.mem.eql(u8, error_name, "invalid_network_policy")) found = true;
    }
    try std.testing.expect(found);
}

test "network policy helper matches URL method headers and string body bytes" {
    var entry_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "origin": "https://api.example.com",
        \\  "methods": ["POST"],
        \\  "pathPrefix": "/v1/",
        \\  "allowedHeaders": ["content-type"],
        \\  "maxRequestBytes": 4
        \\}
    ,
        .{},
    );
    defer entry_json.deinit();
    var params_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "headers": {"content-type": "application/json"},
        \\  "body": "1234"
        \\}
    ,
        .{},
    );
    defer params_json.deinit();

    const parts = try parseNetworkUrlAlloc(std.testing.allocator, "https://api.example.com/v1/status");
    defer freeUrlParts(std.testing.allocator, parts);
    try std.testing.expect(try networkPolicyEntryAllowsRequest(std.testing.allocator, entry_json.value, parts, "POST", params_json.value));
    try std.testing.expect(!(try networkPolicyEntryAllowsRequest(std.testing.allocator, entry_json.value, parts, "GET", params_json.value)));

    var large_body_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "headers": {"content-type": "application/json"},
        \\  "body": "12345"
        \\}
    ,
        .{},
    );
    defer large_body_json.deinit();
    try std.testing.expect(!(try networkPolicyEntryAllowsRequest(std.testing.allocator, entry_json.value, parts, "POST", large_body_json.value)));

    var cookie_entry_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "origin": "https://api.example.com",
        \\  "methods": ["GET"],
        \\  "allowedHeaders": ["cookie"],
        \\  "maxRequestBytes": 64
        \\}
    ,
        .{},
    );
    defer cookie_entry_json.deinit();
    var cookie_params_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "headers": {"cookie": "sid=secret"}
        \\}
    ,
        .{},
    );
    defer cookie_params_json.deinit();
    try std.testing.expect(!(try networkPolicyEntryAllowsRequest(std.testing.allocator, cookie_entry_json.value, parts, "GET", cookie_params_json.value)));

    var credentials_params_json = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        \\{
        \\  "headers": {},
        \\  "credentials": "include"
        \\}
    ,
        .{},
    );
    defer credentials_params_json.deinit();
    try std.testing.expect(!(try networkPolicyEntryAllowsRequest(std.testing.allocator, cookie_entry_json.value, parts, "GET", credentials_params_json.value)));
}
