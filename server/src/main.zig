const std = @import("std");
const core_api = @import("zig_core");

const max_request_bytes = 1024 * 1024;

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

    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.eql(u8, parsed.path, "/webapps/examples")) {
        return writeJson(stream, 200, "{\"ok\":true,\"examples\":[\"notes-lite\",\"task-workbench\",\"file-transformer\",\"api-dashboard\",\"core-replay-lab\"]}");
    }

    return writeJson(stream, 404, "{\"ok\":false,\"error\":{\"code\":\"not_found\",\"message\":\"Route not found\",\"details\":{}}}");
}

fn handleCoreStep(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8) !void {
    _ = allocator;
    const core = core_api.core_create() orelse {
        return writeJson(stream, 500, "{\"ok\":false,\"error\":{\"code\":\"core_create_failed\",\"message\":\"Unable to create Zig core\",\"details\":{}}}");
    };
    defer core_api.core_destroy(core);

    var output: core_api.ZigCoreBuffer = undefined;
    const code = core_api.core_step_json(core, body.ptr, body.len, &output);
    if (code != 0) {
        return writeJson(stream, 500, "{\"ok\":false,\"error\":{\"code\":\"core_step_failed\",\"message\":\"core_step_json failed\",\"details\":{}}}");
    }
    defer core_api.core_free(output);

    return writeJson(stream, 200, output.ptr[0..output.len]);
}

const ParsedRequest = struct {
    method: []const u8,
    path: []const u8,
    body: []const u8,
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
    };
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
