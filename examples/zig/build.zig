// build.zig — Zig build script for the snapdir relay example app.
//
// Builds the `app` executable that imports the snapdir Zig binding module.
// The snapdir module source (src/snapdir.zig + src/c.zig), the vendored C ABI
// header (include/snapdir.h), and the pre-built static lib (lib/libsnapdir_ffi.a)
// are expected alongside this file (staged by run_relay.sh).
//
// Default target: the host architecture (use -Dtarget=aarch64-linux-gnu on
// an x86_64 host to cross-compile for the arm64 libsnapdir_ffi.a).

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target   = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // The snapdir Zig module (same source tree, vendored next to this build.zig).
    const snapdir_mod = b.addModule("snapdir", .{
        .root_source_file = b.path("src/snapdir.zig"),
    });
    // Resolve @cInclude("snapdir.h") in src/c.zig.
    snapdir_mod.addIncludePath(b.path("include"));

    // The relay example app executable.
    const exe = b.addExecutable(.{
        .name             = "app",
        .root_source_file = b.path("app.zig"),
        .target           = target,
        .optimize         = optimize,
    });

    exe.addIncludePath(b.path("include"));
    exe.linkLibC();
    // Link the vendored arm64 ffi staticlib.
    exe.addObjectFile(b.path("lib/libsnapdir_ffi.a"));
    // System libraries required by libsnapdir_ffi.a (Rust runtime deps).
    exe.linkSystemLibrary("pthread");
    exe.linkSystemLibrary("dl");
    exe.linkSystemLibrary("m");
    exe.linkSystemLibrary("gcc_s");
    // Expose the snapdir module to the app.
    exe.root_module.addImport("snapdir", snapdir_mod);

    b.installArtifact(exe);
}
