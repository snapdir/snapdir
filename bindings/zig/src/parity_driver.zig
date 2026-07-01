//! parity_driver.zig — Zig-binding driver for the cross-language parity harness
//! (tests/golden/run_parity.sh, §1 protocol). It exercises the public `snapdir`
//! Zig module (@cImport over the C ABI) and emits BYTE-EXACT stdout:
//!
//!   parity-driver manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
//!   parity-driver id       <path> [--no-follow] [--absolute] [--exclude <RE>]...
//!   parity-driver push     <path> <store_uri> [--jobs N]
//!   parity-driver fetch    <id>   <store_uri>
//!   parity-driver checkout <id>   <store_uri> <dest>
//!
//! stdout is byte-exact per the spec; diagnostics go to stderr; exit 0 = success.
//! The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
//! SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
//! (§1.6); the Zig binding wraps the C ABI → snapdir-api which honors those.
//!
//! Built by the manifest-parity-zig gate (`zig build driver`, aarch64 target +
//! libsnapdir_ffi.a) to tests/golden/drivers/zig-driver-bin. LANE NOTE: this +
//! tests/golden/drivers/zig.sh only CONSUME the binding — never reimplement.

const std = @import("std");
const snapdir = @import("snapdir");

fn die(comptime fmt: []const u8, args: anytype) noreturn {
    std.debug.print("[parity-driver] " ++ fmt ++ "\n", args);
    std.process.exit(1);
}

/// combineExcludes mirrors the Go/C++ drivers' OR-combination of a repeatable
/// `--exclude`: 0 → null, 1 → the pattern as-is, N → `(?:p1)|(?:p2)|…` (the
/// single regex the C ABI accepts).
fn combineExcludes(allocator: std.mem.Allocator, pats: []const []const u8) !?[]const u8 {
    if (pats.len == 0) return null;
    if (pats.len == 1) return pats[0];
    var buf = std.ArrayList(u8).init(allocator);
    for (pats, 0..) |p, i| {
        if (i != 0) try buf.append('|');
        try buf.appendSlice("(?:");
        try buf.appendSlice(p);
        try buf.append(')');
    }
    return try buf.toOwnedSlice();
}

/// parsePathAndOpts parses `<path> [--no-follow] [--absolute] [--exclude <RE>]…`
/// into the path and native snapdir.ManifestOptions.
fn parsePathAndOpts(
    allocator: std.mem.Allocator,
    args: [][:0]u8,
    opts: *snapdir.ManifestOptions,
) ![:0]const u8 {
    var path: ?[:0]const u8 = null;
    var excludes = std.ArrayList([]const u8).init(allocator);
    const ex_eq = "--exclude=";
    var i: usize = 0;
    while (i < args.len) : (i += 1) {
        const a = args[i];
        if (std.mem.eql(u8, a, "--no-follow")) {
            opts.no_follow = true;
        } else if (std.mem.eql(u8, a, "--absolute")) {
            opts.absolute = true;
        } else if (std.mem.eql(u8, a, "--exclude")) {
            i += 1;
            if (i >= args.len) die("--exclude requires an argument", .{});
            try excludes.append(args[i]);
        } else if (std.mem.startsWith(u8, a, ex_eq)) {
            try excludes.append(a[ex_eq.len..]);
        } else if (a.len > 0 and a[0] == '-') {
            die("unknown flag {s}", .{a});
        } else if (path == null) {
            path = a;
        } else {
            die("unexpected extra argument {s}", .{a});
        }
    }
    if (path == null) die("a <path> argument is required", .{});
    opts.exclude = try combineExcludes(allocator, excludes.items);
    return path.?;
}

pub fn main() !void {
    const allocator = std.heap.c_allocator; // short-lived CLI; libc allocator
    const args = try std.process.argsAlloc(allocator);

    if (args.len < 2) die("usage: parity-driver {{manifest|id|push|fetch|checkout}} <args...>", .{});
    const sub = args[1];
    const rest = args[2..];

    var stdout_buf = std.io.bufferedWriter(std.io.getStdOut().writer());
    const out = stdout_buf.writer();

    if (std.mem.eql(u8, sub, "manifest")) {
        var opts = snapdir.ManifestOptions{};
        const path = try parsePathAndOpts(allocator, rest, &opts);
        var m = snapdir.manifest(allocator, path, opts) catch |e| die("manifest failed: {s}", .{@errorName(e)});
        defer m.deinit(allocator);
        // §1.1: emit the raw manifest TEXT byte-exact, incl the trailing \n.
        try out.writeAll(m.raw);
        if (m.raw.len == 0 or m.raw[m.raw.len - 1] != '\n') try out.writeAll("\n");
    } else if (std.mem.eql(u8, sub, "id")) {
        var opts = snapdir.ManifestOptions{};
        const path = try parsePathAndOpts(allocator, rest, &opts);
        const snapid = snapdir.id(allocator, path, opts) catch |e| die("id failed: {s}", .{@errorName(e)});
        try out.writeAll(&snapid); // 64-hex
        try out.writeAll("\n");
    } else if (std.mem.eql(u8, sub, "push")) {
        if (rest.len < 2) die("push requires <path> <store_uri>", .{});
        // push <path> <store_uri> [--jobs N]… (tuning args ignored)
        const snapid = snapdir.push(allocator, rest[0], rest[1], .{}) catch |e| die("push failed: {s}", .{@errorName(e)});
        try out.writeAll(&snapid);
        try out.writeAll("\n");
    } else if (std.mem.eql(u8, sub, "fetch")) {
        if (rest.len < 2) die("fetch requires <id> <store_uri>", .{});
        snapdir.fetch(allocator, rest[0], rest[1], 0) catch |e| die("fetch failed: {s}", .{@errorName(e)});
    } else if (std.mem.eql(u8, sub, "checkout")) {
        // checkout <id> <store_uri> <dest> → pull(id, store, dest)
        if (rest.len < 3) die("checkout requires <id> <store_uri> <dest>", .{});
        snapdir.pull(allocator, rest[0], rest[1], rest[2], .{}) catch |e| die("checkout failed: {s}", .{@errorName(e)});
    } else {
        die("unknown subcommand {s}", .{sub});
    }

    try stdout_buf.flush();
}
