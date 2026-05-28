const std = @import("std");

const max_input_bytes = 1024 * 1024;
const ffi_allocator = std.heap.page_allocator;

pub const ZigCoreBuffer = extern struct {
    ptr: [*]u8,
    len: usize,
};

const Core = struct {
    state_version: u64 = 0,

    fn step(self: *Core, allocator: std.mem.Allocator, input: []const u8) ![]u8 {
        if (input.len > max_input_bytes) {
            return errorJson(allocator, "payload_too_large", "core.step payload exceeds the v0.1 limit");
        }

        var parsed = std.json.parseFromSlice(std.json.Value, allocator, input, .{}) catch {
            return errorJson(allocator, "invalid_json", "core.step input must be valid JSON");
        };
        defer parsed.deinit();

        const root = parsed.value;
        if (root != .object) {
            return errorJson(allocator, "invalid_request", "core.step input must be a JSON object");
        }

        const event_value = root.object.get("event") orelse {
            return errorJson(allocator, "invalid_event", "core.step input requires event");
        };
        if (event_value != .object) {
            return errorJson(allocator, "invalid_event", "event must be an object");
        }

        const type_value = event_value.object.get("type") orelse {
            return errorJson(allocator, "invalid_event", "event.type is required");
        };
        if (type_value != .string) {
            return errorJson(allocator, "invalid_event", "event.type must be a string");
        }

        self.state_version += 1;
        return self.actionsForEvent(allocator, type_value.string, event_value.object.get("payload"));
    }

    fn actionsForEvent(
        self: *Core,
        allocator: std.mem.Allocator,
        event_type: []const u8,
        payload: ?std.json.Value,
    ) ![]u8 {
        if (std.mem.eql(u8, event_type, "CreateTask")) {
            const title = payloadString(payload, "title") orelse "task";
            const escaped_title = try escapeJsonString(allocator, title);
            defer allocator.free(escaped_title);
            return std.fmt.allocPrint(
                allocator,
                "{{\"ok\":true,\"stateVersion\":{},\"actions\":[{{\"type\":\"Toast\",\"message\":\"Task accepted: {s}\",\"level\":\"success\"}},{{\"type\":\"Log\",\"message\":\"CreateTask handled\"}}]}}",
                .{ self.state_version, escaped_title },
            );
        }

        if (std.mem.eql(u8, event_type, "UpdateTask")) {
            return okJson(
                allocator,
                self.state_version,
                "[{\"type\":\"Log\",\"message\":\"UpdateTask handled\"}]",
            );
        }

        if (std.mem.eql(u8, event_type, "TransformText")) {
            const text = payloadString(payload, "text") orelse "";
            const mode = payloadString(payload, "mode") orelse "uppercase";
            const transformed = try transformText(allocator, text, mode);
            defer allocator.free(transformed);
            const escaped = try escapeJsonString(allocator, transformed);
            defer allocator.free(escaped);
            return std.fmt.allocPrint(
                allocator,
                "{{\"ok\":true,\"stateVersion\":{},\"actions\":[{{\"type\":\"TransformText\",\"text\":\"{s}\"}}]}}",
                .{ self.state_version, escaped },
            );
        }

        if (std.mem.eql(u8, event_type, "ImportFile")) {
            return okJson(
                allocator,
                self.state_version,
                "[{\"type\":\"Log\",\"message\":\"ImportFile handled\"}]",
            );
        }

        if (std.mem.eql(u8, event_type, "NetworkSnapshotReceived")) {
            return okJson(
                allocator,
                self.state_version,
                "[{\"type\":\"RenderHint\",\"hint\":\"network-snapshot-received\"}]",
            );
        }

        if (std.mem.eql(u8, event_type, "ReplayEvents")) {
            return okJson(
                allocator,
                self.state_version,
                "[{\"type\":\"Log\",\"message\":\"ReplayEvents handled\"}]",
            );
        }

        const escaped_type = try escapeJsonString(allocator, event_type);
        defer allocator.free(escaped_type);
        return std.fmt.allocPrint(
            allocator,
            "{{\"ok\":true,\"stateVersion\":{},\"actions\":[{{\"type\":\"Log\",\"message\":\"Unhandled event: {s}\"}}]}}",
            .{ self.state_version, escaped_type },
        );
    }
};

export fn core_create() ?*Core {
    const core = ffi_allocator.create(Core) catch return null;
    core.* = .{};
    return core;
}

export fn core_destroy(core: ?*Core) void {
    if (core) |ptr| {
        ffi_allocator.destroy(ptr);
    }
}

export fn core_step_json(core: ?*Core, input_ptr: ?[*]const u8, input_len: usize, output: ?*ZigCoreBuffer) i32 {
    if (core == null or input_ptr == null or output == null) {
        return -1;
    }

    const input = input_ptr.?[0..input_len];
    const bytes = core.?.step(ffi_allocator, input) catch return -2;
    output.?.* = .{
        .ptr = bytes.ptr,
        .len = bytes.len,
    };
    return 0;
}

export fn core_free(buffer: ZigCoreBuffer) void {
    if (buffer.len == 0) return;
    ffi_allocator.free(buffer.ptr[0..buffer.len]);
}

fn okJson(allocator: std.mem.Allocator, state_version: u64, actions_json: []const u8) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":true,\"stateVersion\":{},\"actions\":{s}}}",
        .{ state_version, actions_json },
    );
}

fn errorJson(allocator: std.mem.Allocator, code: []const u8, message: []const u8) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\"}},\"actions\":[]}}",
        .{ code, message },
    );
}

fn payloadString(payload: ?std.json.Value, field: []const u8) ?[]const u8 {
    const value = payload orelse return null;
    if (value != .object) return null;
    const field_value = value.object.get(field) orelse return null;
    if (field_value != .string) return null;
    return field_value.string;
}

fn transformText(allocator: std.mem.Allocator, text: []const u8, mode: []const u8) ![]u8 {
    if (std.mem.eql(u8, mode, "lowercase")) {
        const out = try allocator.alloc(u8, text.len);
        for (text, 0..) |char, index| {
            out[index] = std.ascii.toLower(char);
        }
        return out;
    }

    if (std.mem.eql(u8, mode, "reverse-lines")) {
        var parts = std.mem.splitScalar(u8, text, '\n');
        var lines: std.ArrayList([]const u8) = .empty;
        defer lines.deinit(allocator);
        while (parts.next()) |line| {
            try lines.append(allocator, line);
        }
        var out: std.ArrayList(u8) = .empty;
        for (0..lines.items.len) |offset| {
            if (offset > 0) try out.append(allocator, '\n');
            const index = lines.items.len - 1 - offset;
            try out.appendSlice(allocator, lines.items[index]);
        }
        return out.toOwnedSlice(allocator);
    }

    if (std.mem.eql(u8, mode, "word-count")) {
        const words = countWords(text);
        const lines = if (text.len == 0) 0 else std.mem.count(u8, text, "\n") + 1;
        return std.fmt.allocPrint(
            allocator,
            "Words: {}\nLines: {}\nCharacters: {}",
            .{ words, lines, text.len },
        );
    }

    const out = try allocator.alloc(u8, text.len);
    for (text, 0..) |char, index| {
        out[index] = std.ascii.toUpper(char);
    }
    return out;
}

fn countWords(text: []const u8) usize {
    var words: usize = 0;
    var in_word = false;
    for (text) |char| {
        if (std.ascii.isWhitespace(char)) {
            in_word = false;
        } else if (!in_word) {
            words += 1;
            in_word = true;
        }
    }
    return words;
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

test "FFI create step free destroy" {
    const core = core_create() orelse return error.OutOfMemory;
    defer core_destroy(core);

    const input =
        \\{"app":"task-workbench","event":{"type":"CreateTask","payload":{"title":"Fixture task"}}}
    ;
    var output: ZigCoreBuffer = undefined;
    try std.testing.expectEqual(@as(i32, 0), core_step_json(core, input.ptr, input.len, &output));
    defer core_free(output);

    const json = output.ptr[0..output.len];
    try std.testing.expect(std.mem.indexOf(u8, json, "\"ok\":true") != null);
    try std.testing.expect(std.mem.indexOf(u8, json, "Task accepted") != null);
}

test "invalid JSON returns structured logical error" {
    var core = Core{};
    const json = try core.step(std.testing.allocator, "{bad json");
    defer std.testing.allocator.free(json);

    try std.testing.expect(std.mem.indexOf(u8, json, "\"ok\":false") != null);
    try std.testing.expect(std.mem.indexOf(u8, json, "invalid_json") != null);
}

test "same initial state and event produce deterministic JSON" {
    const input =
        \\{"app":"file-transformer","event":{"type":"TransformText","payload":{"text":"Hello","mode":"lowercase"}}}
    ;
    var a = Core{};
    var b = Core{};
    const out_a = try a.step(std.testing.allocator, input);
    defer std.testing.allocator.free(out_a);
    const out_b = try b.step(std.testing.allocator, input);
    defer std.testing.allocator.free(out_b);

    try std.testing.expectEqualStrings(out_a, out_b);
    try std.testing.expect(std.mem.indexOf(u8, out_a, "\"text\":\"hello\"") != null);
}

test "payload over limit fails safely" {
    var core = Core{};
    const input = try std.testing.allocator.alloc(u8, max_input_bytes + 1);
    defer std.testing.allocator.free(input);
    @memset(input, ' ');

    const json = try core.step(std.testing.allocator, input);
    defer std.testing.allocator.free(json);

    try std.testing.expect(std.mem.indexOf(u8, json, "payload_too_large") != null);
}
