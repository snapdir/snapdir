//! app.zig — canonical example: snapdir Zig binding CLI
//!
//! Demonstrates the snapdir Zig binding API over a shared S3 store.
//! The store URI and credentials are read from the environment:
//!   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
//!
//! CLI:
//!   app push <dir> <store>              — prints the 64-hex snapshot id
//!   app pull <id>  <store> <dest>       — materialises snapshot into dest
//!   app id   <dir>                      — prints the 64-hex snapshot id
//!   app diff <store@id_a> <store@id_b>  — prints STATUS<TAB>PATH per line

const std = @import("std");
const snapdir = @import("snapdir");

const stdout = std.io.getStdOut().writer();
const stderr = std.io.getStdErr().writer();

/// Split "store@id" into { store, id }.  The last '@' is the delimiter.
fn parseRef(ref: []const u8) struct { store: []const u8, id: []const u8 } {
    const at = std.mem.lastIndexOfScalar(u8, ref, '@') orelse return .{ .store = ref, .id = "" };
    return .{ .store = ref[0..at], .id = ref[at + 1 ..] };
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);

    if (args.len < 2) {
        try stderr.writeAll("usage: app {push|pull|id|diff} [args...]\n");
        std.process.exit(1);
    }

    const cmd = args[1];

    if (std.mem.eql(u8, cmd, "push")) {
        // push <dir> <store> — stage dir and upload to store; print snapshot id.
        const dir   = try allocator.dupeZ(u8, args[2]);
        defer allocator.free(dir);
        const store = try allocator.dupeZ(u8, args[3]);
        defer allocator.free(store);

        const id = try snapdir.push(allocator, dir, store, .{});
        try stdout.print("{s}\n", .{id});

    } else if (std.mem.eql(u8, cmd, "pull")) {
        // pull <id> <store> <dest> — fetch snapshot from store and materialise.
        const id    = try allocator.dupeZ(u8, args[2]);
        defer allocator.free(id);
        const store = try allocator.dupeZ(u8, args[3]);
        defer allocator.free(store);
        const dest  = try allocator.dupeZ(u8, args[4]);
        defer allocator.free(dest);

        try snapdir.pull(allocator, id, store, dest, .{});

    } else if (std.mem.eql(u8, cmd, "id")) {
        // id <dir> — compute and print the snapshot id for dir.
        const dir = try allocator.dupeZ(u8, args[2]);
        defer allocator.free(dir);

        const id = try snapdir.id(allocator, dir, .{});
        try stdout.print("{s}\n", .{id});

    } else if (std.mem.eql(u8, cmd, "diff")) {
        // diff <store@id_a> <store@id_b> — compare two pinned snapshots.
        //
        // The binding's diff() compares two STORE contents. To diff two pinned
        // snapshots from the same store we pull each into a temporary directory,
        // push each to its own temporary file store, then diff those two stores.
        const ref_from = parseRef(args[2]);
        const ref_to   = parseRef(args[3]);

        const store_from = try allocator.dupeZ(u8, ref_from.store);
        defer allocator.free(store_from);
        const id_from    = try allocator.dupeZ(u8, ref_from.id);
        defer allocator.free(id_from);
        const store_to   = try allocator.dupeZ(u8, ref_to.store);
        defer allocator.free(store_to);
        const id_to      = try allocator.dupeZ(u8, ref_to.id);
        defer allocator.free(id_to);

        // Create temp dirs in /tmp (container-local; no cleanup needed on exit).
        const dir_from:   [:0]const u8 = "/tmp/sd-zig-diff-from";
        const dir_to:     [:0]const u8 = "/tmp/sd-zig-diff-to";
        const fstore_from: [:0]const u8 = "file:///tmp/sd-zig-store-from";
        const fstore_to:   [:0]const u8 = "file:///tmp/sd-zig-store-to";

        std.fs.makeDirAbsolute(dir_from)  catch |err| if (err != error.PathAlreadyExists) return err;
        std.fs.makeDirAbsolute(dir_to)    catch |err| if (err != error.PathAlreadyExists) return err;

        try snapdir.pull(allocator, id_from, store_from, dir_from, .{});
        _ = try snapdir.push(allocator, dir_from, fstore_from, .{});
        try snapdir.pull(allocator, id_to, store_to, dir_to, .{});
        _ = try snapdir.push(allocator, dir_to, fstore_to, .{});

        const entries = try snapdir.diff(allocator, fstore_from, fstore_to, .{});
        defer {
            for (entries) |e| allocator.free(e.path);
            allocator.free(entries);
        }

        // Print as STATUS<TAB>PATH per line — matches the snapdir CLI diff format.
        for (entries) |e| {
            try stdout.print("{c}\t{s}\n", .{ @intFromEnum(e.status), e.path });
        }

    } else {
        try stderr.print("unknown command: {s}\n", .{cmd});
        std.process.exit(1);
    }
}
