const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const core_module = b.createModule(.{
        .root_source_file = b.path("../zig-core/src/lib.zig"),
        .target = target,
        .optimize = optimize,
    });

    const server_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "zig_core", .module = core_module },
        },
    });

    const server = b.addExecutable(.{
        .name = "native-ai-server",
        .root_module = server_module,
    });
    server.linkLibC();
    b.installArtifact(server);

    const run_server = b.addRunArtifact(server);
    if (b.args) |args| {
        run_server.addArgs(args);
    }
    const run_step = b.step("run-server", "Run the Zig HTTP server");
    run_step.dependOn(&run_server.step);

    const tests = b.addTest(.{
        .root_module = server_module,
    });
    tests.linkLibC();
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Build and run server unit tests");
    test_step.dependOn(&run_tests.step);
}
