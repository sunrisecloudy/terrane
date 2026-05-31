const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const static_module = b.createModule(.{
        .root_source_file = b.path("src/lib.zig"),
        .target = target,
        .optimize = optimize,
    });

    const lib = b.addLibrary(.{
        .name = "zig_crdt",
        .root_module = static_module,
        .linkage = .static,
    });
    lib.linkLibC();
    b.installArtifact(lib);

    const dynamic_module = b.createModule(.{
        .root_source_file = b.path("src/lib.zig"),
        .target = target,
        .optimize = optimize,
    });

    const dylib = b.addLibrary(.{
        .name = "zig_crdt",
        .root_module = dynamic_module,
        .linkage = .dynamic,
    });
    dylib.linkLibC();
    b.installArtifact(dylib);

    const tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/lib.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    tests.linkLibC();

    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run Zig CRDT unit tests");
    test_step.dependOn(&run_tests.step);
}
