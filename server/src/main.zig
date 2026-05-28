const std = @import("std");
const core_api = @import("zig_core");

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
        return handleBridge(allocator, stream, parsed.body, parsed.app_id);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/webapps/validate")) {
        return handleWebappValidate(allocator, stream, parsed.body);
    }

    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.eql(u8, parsed.path, "/webapps/examples")) {
        return writeJson(stream, 200, "{\"ok\":true,\"examples\":[\"notes-lite\",\"task-workbench\",\"file-transformer\",\"api-dashboard\",\"core-replay-lab\"]}");
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

fn handleBridge(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8, app_id: ?[]const u8) !void {
    const channel_app_id = app_id orelse {
        return writeBridgeError(allocator, stream, "unknown", "bridge.unauthorized_channel", "Bridge calls require a channel-derived app id");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        return writeBridgeError(allocator, stream, "unknown", "invalid_request", "Bridge request body must be valid JSON");
    };
    defer parsed.deinit();

    const root = parsed.value;
    if (root != .object) {
        return writeBridgeError(allocator, stream, "unknown", "invalid_request", "Bridge request body must be an object");
    }

    const id = valueString(root.object.get("id")) orelse "unknown";
    const method = valueString(root.object.get("method")) orelse {
        return writeBridgeError(allocator, stream, id, "invalid_request", "Bridge request method must be a string");
    };

    if (std.mem.eql(u8, method, "core.step")) {
        const params = root.object.get("params") orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "core.step requires params");
        };
        if (params != .object) {
            return writeBridgeError(allocator, stream, id, "invalid_request", "core.step params must be an object");
        }
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

    if (isKnownUnsupportedBridgeMethod(method)) {
        return writeBridgeError(allocator, stream, id, "platform_unsupported", "Bridge method is not implemented on zig-server");
    }

    return writeBridgeError(allocator, stream, id, "unknown_method", "Unknown bridge method");
}

fn handleWebappValidate(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8) !void {
    const report = try validateWebappPackage(allocator, body);
    defer allocator.free(report);
    return writeJson(stream, 200, report);
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

const ParsedRequest = struct {
    method: []const u8,
    path: []const u8,
    body: []const u8,
    app_id: ?[]const u8,
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
    const reason = switch (status) {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        else => "OK",
    };
    var header_buffer: [256]u8 = undefined;
    const header = try std.fmt.bufPrint(
        &header_buffer,
        "HTTP/1.1 {d} {s}\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {d}\r\nconnection: close\r\n\r\n",
        .{ status, reason, body.len },
    );
    try stream.writeAll(header);
    try stream.writeAll(body);
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

fn serverCapabilitiesJson(allocator: std.mem.Allocator) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"runtimeVersion\":\"{s}\",\"platform\":\"server\",\"target\":\"zig-server\",\"devMode\":false,\"features\":{{\"core.step\":true,\"runtime.capabilities\":true,\"storage.get\":false,\"storage.set\":false,\"storage.remove\":false,\"storage.list\":false,\"dialog.openFile\":false,\"dialog.saveFile\":false,\"notification.toast\":false,\"network.request\":false,\"app.log\":false}},\"limits\":{{\"maxPackageBytes\":1048576,\"maxFileBytes\":524288}}}}",
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
        "storage.get",
        "storage.set",
        "storage.remove",
        "storage.list",
        "dialog.openFile",
        "dialog.saveFile",
        "notification.toast",
        "network.request",
        "app.log",
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
