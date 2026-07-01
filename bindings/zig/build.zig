// build.zig — Zig build script for the snapdir binding.
//
// Default step (`zig build`): builds the snapdir static library module.
// This resolves the @cImport on bindings/zig/include/snapdir.h WITHOUT
// linking libsnapdir_ffi.a — the translate-c step only needs the header.
//
// ─── IMPORTANT: linking libsnapdir_ffi.a for tests / executables ─────────────
//
// When a later gate adds a test binary or executable that actually CALLS the C
// functions at runtime, it must:
//
//   1. Target aarch64-linux-gnu to match the arm64 libsnapdir_ffi.a:
//        const target = b.resolveTargetQuery(.{
//            .cpu_arch = .aarch64,
//            .os_tag   = .linux,
//            .abi      = .gnu,
//        });
//
//   2. Link libc + the ffi static library + system libs:
//        exe.linkLibC();
//        exe.addObjectFile(b.path("lib/libsnapdir_ffi.a"));
//        exe.linkSystemLibrary("pthread");
//        exe.linkSystemLibrary("dl");
//        exe.linkSystemLibrary("m");
//
//   3. The vendored header and staticlib must be present at:
//        bindings/zig/include/snapdir.h   (vendored from include/snapdir.h)
//        bindings/zig/lib/libsnapdir_ffi.a  (vendored from target/release/)
//
//   The verification step vendors both before running `zig build`.
//
// ─────────────────────────────────────────────────────────────────────────────

const std = @import("std");

pub fn build(b: *std.Build) void {
    const optimize = b.standardOptimizeOption(.{});
    const target   = b.standardTargetOptions(.{});

    // ── snapdir static library module ────────────────────────────────────────
    //
    // Build the snapdir Zig module as a static library.  The default `zig build`
    // step resolves the @cImport in src/c.zig by running translate-c on
    // bindings/zig/include/snapdir.h.  No arm64 ffi staticlib link is needed
    // for this step — translate-c only needs the header file.

    const lib = b.addStaticLibrary(.{
        .name         = "snapdir",
        .root_source_file = b.path("src/snapdir.zig"),
        .target       = target,
        .optimize     = optimize,
    });

    // Resolve @cInclude("snapdir.h") in src/c.zig to the vendored header.
    lib.addIncludePath(b.path("include"));

    b.installArtifact(lib);

    // ── test step ────────────────────────────────────────────────────────────
    //
    // The test binary targets <test-arch>-linux-gnu to match the vendored
    // libsnapdir_ffi.a.  The arch is controlled by the `-Dtest-arch=` build
    // option (default "aarch64" to preserve dev-image behavior; pass
    // `-Dtest-arch=x86_64` on x86_64 GH runners where cargo builds an x86_64
    // staticlib).  In the dev image (amd64-emulated, arm64 kernel) aarch64
    // executes natively via binfmt_misc.
    //
    // Link order: libsnapdir_ffi.a (static) + libc + pthread + dl + m.
    // The snapdir module is exposed as an import so `@import("snapdir")` works.

    const test_arch_name = b.option([]const u8, "test-arch", "cpu arch for the FFI-linked test/driver binaries (matches the vendored libsnapdir_ffi.a)") orelse "aarch64";
    const test_cpu_arch = std.meta.stringToEnum(std.Target.Cpu.Arch, test_arch_name) orelse .aarch64;
    const test_target = b.resolveTargetQuery(.{
        .cpu_arch = test_cpu_arch,
        .os_tag   = .linux,
        .abi      = .gnu,
    });

    // Create the snapdir module so `@import("snapdir")` works in the test.
    const snapdir_mod = b.addModule("snapdir", .{
        .root_source_file = b.path("src/snapdir.zig"),
    });
    snapdir_mod.addIncludePath(b.path("include"));

    const unit_tests = b.addTest(.{
        .name             = "snapdir-test",
        .root_source_file = b.path("test/zig_api.zig"),
        .target           = test_target,
        .optimize         = optimize,
    });

    // Include path for @cInclude in c.zig (needed by the snapdir module).
    unit_tests.addIncludePath(b.path("include"));
    // Link libc (required by the C ABI and setenv usage in the tests).
    unit_tests.linkLibC();
    // fchmod_compat.c: Zig 0.13 opens dirs without iterate=true using O_PATH;
    // fchmod(O_PATH_fd) returns EBADF which Zig treats as unreachable (panic).
    // This shim retries via /proc/self/fd/<n> so Dir.chmod works on O_PATH fds.
    // Must be added BEFORE the ffi staticlib so the shim's fchmod wins.
    unit_tests.addCSourceFile(.{
        .file = b.path("src/fchmod_compat.c"),
        .flags = &.{"-D_GNU_SOURCE"},
    });
    // Link the vendored arm64 ffi staticlib.
    unit_tests.addObjectFile(b.path("lib/libsnapdir_ffi.a"));
    // System libraries required by libsnapdir_ffi.a (Rust runtime deps).
    unit_tests.linkSystemLibrary("pthread");
    unit_tests.linkSystemLibrary("dl");
    unit_tests.linkSystemLibrary("m");
    // libgcc_s supplies _Unwind_* symbols that the Rust staticlib depends on.
    unit_tests.linkSystemLibrary("gcc_s");
    // Expose the snapdir module to the test file.
    unit_tests.root_module.addImport("snapdir", snapdir_mod);

    const run_unit_tests = b.addRunArtifact(unit_tests);

    const test_step = b.step("test", "Run snapdir unit tests");
    test_step.dependOn(&run_unit_tests.step);

    // ── parity driver executable (`zig build driver`) ────────────────────────
    //
    // A CLI over the binding implementing the cross-language parity §1 protocol
    // (manifest/id/push/fetch/checkout). Same `-Dtest-arch` target + ffi link
    // as the test binary (it calls the C ABI at runtime); it does NOT need
    // fchmod_compat.c (no Dir.chmod path). The manifest-parity-zig gate builds
    // this and copies zig-out/bin/snapdir-parity-driver to
    // tests/golden/drivers/zig-driver-bin, which tests/golden/drivers/zig.sh execs.
    const driver = b.addExecutable(.{
        .name = "snapdir-parity-driver",
        .root_source_file = b.path("src/parity_driver.zig"),
        .target = test_target,
        .optimize = optimize,
    });
    driver.addIncludePath(b.path("include"));
    driver.linkLibC();
    driver.addObjectFile(b.path("lib/libsnapdir_ffi.a"));
    driver.linkSystemLibrary("pthread");
    driver.linkSystemLibrary("dl");
    driver.linkSystemLibrary("m");
    driver.linkSystemLibrary("gcc_s");
    driver.root_module.addImport("snapdir", snapdir_mod);

    const driver_step = b.step("driver", "Build the parity-harness driver executable");
    driver_step.dependOn(&b.addInstallArtifact(driver, .{}).step);
}
