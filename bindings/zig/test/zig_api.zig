// zig_api.zig — black-box spec for the snapdir idiomatic Zig binding
// (Phase 41, gate zig-api-spec-tests; adversary/opus).
//
// Authored from the SPEC ONLY: the PUBLIC declarations of
// bindings/zig/src/snapdir.zig + the FROZEN C ABI (include/snapdir.h,
// c-abi.sha.lock). NO visibility into src/c.zig internals or the Rust src/.
// These are EXTERNAL behavioural tests against the public `snapdir` module;
// they are EXPECTED TO FAIL / be incomplete against the current scaffold and
// are made green by zig-api-impl. Do NOT weaken an assertion to pass the
// (unproven) scaffold.
//
// Wiring (zig-api-impl): this file is `git mv`'d into the binding's test dir
// and the public surface is imported as a module named "snapdir":
//
//     const snapdir = @import("snapdir");
//
// The impl gate builds the test binary for aarch64-linux-gnu and links
// libsnapdir_ffi.a + libc + pthread/dl/m (see build.zig NOTE). All tests run
// under `std.testing.allocator` — its leak detector is the PARAMOUNT mechanism
// here (the Zig analogue of valgrind for the C++ lane): a missing
// `Manifest.deinit` / un-freed `diff` slice trips it and FAILS the test.
//
// Pinned PUBLIC surface (the impl MUST satisfy these signatures):
//   pub fn version(allocator) ![]u8                    // caller frees
//   pub fn manifest(allocator, path:[:0]const u8, ManifestOptions) !Manifest
//   pub fn id(allocator, path:[:0]const u8, ManifestOptions) ![64]u8   // value, no free
//   pub fn idFromManifest(m: Manifest) ![64]u8
//   pub fn push(allocator, path, store_uri:[:0]const u8, PushOptions) ![64]u8
//   pub fn pull(allocator, id, store_uri, dest:[:0]const u8, PullOptions) !void
//   pub fn fetch(allocator, id, store_uri:[:0]const u8, jobs:u32) !void
//   pub fn diff(allocator, from_uri, to_uri:[:0]const u8, DiffOptions) ![]DiffEntry
//   SnapdirError = error{IoError,HashMismatch,StoreError,InFlux,CatalogError,
//                        InvalidId,InvalidStore,Conflict,OutOfMemory}
//   ManifestEntry{type:PathType, perm:u32, checksum:[64]u8, size:u64, path:[]const u8}
//   Manifest{ raw:[]u8, entries:[]ManifestEntry, pub fn deinit(self,allocator) }
//   DiffEntry{ status:DiffStatus, path:[]const u8 }
//   PathType{File='F',Directory='D',Symlink='L'}; DiffStatus{A,D,M,=}

const std = @import("std");
const snapdir = @import("snapdir");
const testing = std.testing;

// libc setenv — the binding links libc, so we drive the hermetic/offline env
// (cache + catalog under a temp dir) directly. zig 0.13 has no stable
// std.posix.setenv; this is the portable, dependency-free way and keeps every
// snapdir_* call from touching $HOME / a network catalog.
extern "c" fn setenv(name: [*:0]const u8, value: [*:0]const u8, overwrite: c_int) c_int;

// ─── black-box helpers ──────────────────────────────────────────────────────

/// hex64 — true iff `s` is exactly 64 LOWERCASE hex characters. snapshot ids
/// and file checksums are pinned to this shape by the C ABI.
fn isHex64(s: []const u8) bool {
    if (s.len != 64) return false;
    for (s) |ch| {
        const ok = (ch >= '0' and ch <= '9') or (ch >= 'a' and ch <= 'f');
        if (!ok) return false;
    }
    return true;
}

/// The 8 stable ABI error codes mapped into the Zig SnapdirError set. A binding
/// failure MUST surface a member of this set (errors are RETURNED, never
/// panicked). OutOfMemory stands in for the C "INTERNAL" catch-all.
fn isStableSnapdirError(e: anyerror) bool {
    return switch (e) {
        error.IoError,
        error.HashMismatch,
        error.StoreError,
        error.InFlux,
        error.CatalogError,
        error.InvalidId,
        error.InvalidStore,
        error.Conflict,
        error.OutOfMemory,
        => true,
        else => false,
    };
}

/// makeOfflineEnv points the cache + catalog at a freshly-created temp dir so
/// no test reaches the network or the user's real cache. Called once per test
/// that performs snapdir operations. The returned tmp dir must be `.cleanup()`d.
fn makeOfflineEnv() !std.testing.TmpDir {
    var tmp = testing.tmpDir(.{});
    errdefer tmp.cleanup();

    // Realpath of the tmp dir so the env values are absolute & NUL-terminated.
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const base = try tmp.dir.realpath(".", &buf);

    var cache_buf: [std.fs.max_path_bytes]u8 = undefined;
    var cat_buf: [std.fs.max_path_bytes]u8 = undefined;
    const cache_z = try std.fmt.bufPrintZ(&cache_buf, "{s}/cache", .{base});
    const cat_z = try std.fmt.bufPrintZ(&cat_buf, "{s}/catalog.db", .{base});

    _ = setenv("SNAPDIR_CACHE_DIR", cache_z.ptr, 1);
    _ = setenv("SNAPDIR_CATALOG_DB_PATH", cat_z.ptr, 1);
    // Suppress any catalog adapter that might phone a network location.
    _ = setenv("SNAPDIR_CATALOG", "none", 1);
    return tmp;
}

/// buildTree writes a small offline tree under `dir`:
///   a.txt        (0o644, "hello")
///   sub/         (0o755)
///   sub/b.bin    (0o600, "world!\n")
/// Permissions are non-default on purpose so the round-trip restore contract
/// is observable. Returns the absolute, NUL-terminated path of the root.
fn buildTree(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    // Build the snapshot tree in a DEDICATED "tree" subdir of the test's tmp
    // root. The cache (SNAPDIR_CACHE_DIR=<tmp>/cache), catalog.db, store/ and
    // dest/ all live as SIBLINGS under <tmp> — never inside the snapshot tree.
    // (PM hermeticity re-address, zig-api-impl: the helper previously built the
    // tree directly in <tmp>, so push() snapshotted a clean tree while a later
    // id() ALSO walked the cache/store objects push had written into <tmp>,
    // diverging the ids. That is a test-setup flaw, not an impl bug — push-id ==
    // id holds in the core, as the cpp/go round-trip tests prove. No assertion
    // changed; only the tree is isolated from the sibling artifacts.)
    var tree = try dir.makeOpenPath("tree", .{});
    defer tree.close();
    try tree.writeFile(.{ .sub_path = "a.txt", .data = "hello" });
    try tree.makePath("sub");
    try tree.writeFile(.{ .sub_path = "sub/b.bin", .data = "world!\n" });
    // Best-effort non-default perms (the FS may flatten these; the round-trip
    // test only relies on the perms snapdir actually records).
    if (tree.openFile("a.txt", .{})) |f| {
        f.chmod(0o644) catch {};
        f.close();
    } else |_| {}
    if (tree.openFile("sub/b.bin", .{})) |f| {
        f.chmod(0o600) catch {};
        f.close();
    } else |_| {}

    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try tree.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "{s}", .{real});
}

/// rmrf robustly removes a tmp dir's contents via `rm -rf`. Unlike zig's
/// std `deleteTree` — which panics ("reached unreachable code") on snapdir's
/// content-addressed object store + a pulled-into restrictive dest dir — `rm`
/// tolerates any FS layout/perms. Best-effort; registered as a `defer` that
/// runs BEFORE the test's `tmp.cleanup()` so the subsequent deleteTree finds an
/// empty tree and its own `catch {}` swallows the FileNotFound. (PM teardown
/// workaround for zig-api-impl — operator-approved; binding logic unchanged.)
fn rmrf(allocator: std.mem.Allocator, dir: std.fs.Dir) void {
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const path = dir.realpath(".", &buf) catch return;
    const path_owned = allocator.dupe(u8, path) catch return;
    defer allocator.free(path_owned);
    var child = std.process.Child.init(&.{ "rm", "-rf", path_owned }, allocator);
    _ = child.spawnAndWait() catch {};
}

/// fileUri builds a NUL-terminated "file://<abs>" store URI for an offline
/// file store rooted at `dir`'s realpath. Caller frees.
fn fileUri(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try dir.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "file://{s}", .{real});
}

// ─── 1. allocator / leak discipline (THE PARAMOUNT AXIS) ────────────────────
//
// Every owned-memory call is exercised under std.testing.allocator so a leak
// FAILS the test: version() → free; manifest() → Manifest.deinit; diff() →
// free each .path + the slice. id()/idFromManifest() return a [64]u8 VALUE
// (no free — exercised to prove they don't secretly leak through a helper).
// manifest+deinit is looped 50× so a per-call leak is unmistakable.

test "version returns a caller-owned slice that must be freed (no leak)" {
    const a = testing.allocator;
    const v = try snapdir.version(a);
    defer a.free(v); // a MISSING free here trips testing.allocator → FAIL
    // version is a non-empty string (e.g. "1.10.0"-shaped); we don't over-fit
    // the exact value, only that it is owned, non-empty, and printable ASCII.
    try testing.expect(v.len > 0);
    for (v) |ch| try testing.expect(ch >= 0x20 and ch < 0x7f);
}

test "manifest+deinit is leak-free across 50 iterations (paramount)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var i: usize = 0;
    while (i < 50) : (i += 1) {
        var m = try snapdir.manifest(a, root, .{});
        // If deinit fails to free raw/entries/paths, testing.allocator reports
        // a leak at test teardown and the whole test FAILS.
        defer m.deinit(a);
        try testing.expect(m.entries.len > 0);
        try testing.expect(m.raw.len > 0);
    }
}

test "diff result slice and each path are owned and freed (no leak)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    _ = try snapdir.push(a, root, store_uri, .{});

    // self-diff (same store on both sides). Whatever entries come back, the
    // slice AND every .path must be allocator-owned: we free them all and a
    // leak (or a double-free / borrowed pointer) trips testing.allocator.
    const entries = try snapdir.diff(a, store_uri, store_uri, .{});
    defer {
        for (entries) |e| a.free(e.path);
        a.free(entries);
    }
    // A strict (non-unchanged) self-diff must contain no A/D/M rows.
    for (entries) |e| {
        try testing.expect(e.status == .Added or e.status == .Deleted or
            e.status == .Modified or e.status == .Unchanged);
        try testing.expect(e.status == .Unchanged);
    }
}

// ─── 2. checksum is a fixed [64]u8 array, never a heap slice ────────────────

test "ManifestEntry.checksum is a compile-time [64]u8 (not a slice)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var m = try snapdir.manifest(a, root, .{});
    defer m.deinit(a);

    // COMPILE-PIN: checksum must be a fixed [64]u8 array. If the impl ever made
    // it a []const u8 this @TypeOf comparison fails to compile.
    comptime {
        const FieldT = @TypeOf(@as(snapdir.ManifestEntry, undefined).checksum);
        if (FieldT != [64]u8) @compileError("ManifestEntry.checksum must be [64]u8");
        const PermT = @TypeOf(@as(snapdir.ManifestEntry, undefined).perm);
        if (PermT != u32) @compileError("ManifestEntry.perm must be u32");
        const SizeT = @TypeOf(@as(snapdir.ManifestEntry, undefined).size);
        if (SizeT != u64) @compileError("ManifestEntry.size must be u64");
    }

    // A regular file entry (a.txt) must carry a 64 lowercase-hex checksum.
    var saw_file = false;
    var saw_dir = false;
    for (m.entries) |e| {
        try testing.expect(e.path.len > 0);
        if (e.type == .File) {
            try testing.expect(isHex64(&e.checksum));
            saw_file = true;
        } else if (e.type == .Directory) {
            saw_dir = true;
        }
        // size really is u64-wide (a value past u32 must not overflow).
        const huge = e.size +% (@as(u64, 1) << 40);
        try testing.expect(huge >= e.size);
    }
    try testing.expect(saw_file); // a.txt + sub/b.bin
    try testing.expect(saw_dir); // sub/
}

// ─── 3. id / manifest self-consistency + determinism ────────────────────────

test "id is 64-hex value, deterministic, == idFromManifest(manifest)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    // id() returns a [64]u8 VALUE — no allocation, nothing to free.
    const id1 = try snapdir.id(a, root, .{});
    comptime {
        if (@TypeOf(id1) != [64]u8) @compileError("id() must return [64]u8");
    }
    try testing.expect(isHex64(&id1));

    // Determinism: a second id() over the unchanged tree is byte-identical.
    const id2 = try snapdir.id(a, root, .{});
    try testing.expectEqualSlices(u8, &id1, &id2);

    // idFromManifest(manifest(tree)) == id(tree) — the pure/sync cross-check.
    var m = try snapdir.manifest(a, root, .{});
    defer m.deinit(a);
    const id_fm = try snapdir.idFromManifest(m);
    try testing.expect(isHex64(&id_fm));
    try testing.expectEqualSlices(u8, &id1, &id_fm);
}

// ─── 4. error mapping: failures RETURN a SnapdirError, never @panic ─────────

test "id/manifest on a missing path return a SnapdirError (not a panic)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    const missing: [:0]const u8 = "/snapdir/no/such/path/zzz-does-not-exist-xyz";

    // id() must RETURN an error from the set (IoError is the canonical
    // missing-path code) — a reachable @panic here would crash the test binary.
    if (snapdir.id(a, missing, .{})) |_| {
        try testing.expect(false); // must not succeed on a missing path
    } else |err| {
        try testing.expect(isStableSnapdirError(err));
    }

    // manifest() — a different C entry point, same returned-error contract.
    if (snapdir.manifest(a, missing, .{})) |*m_ok| {
        var m = m_ok.*;
        m.deinit(a);
        try testing.expect(false);
    } else |err| {
        try testing.expect(isStableSnapdirError(err));
    }
}

test "bogus store URIs return a SnapdirError, not a panic" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    // An unknown/garbage scheme is a genuinely invalid store → InvalidStore.
    // (A *missing but valid* file:// store is NOT an error — snapdir treats an
    // absent store as empty — so we pin the typed error on a bad scheme.)
    if (snapdir.push(a, root, "ftp://not-a-store/x", .{})) |_| {
        try testing.expect(false);
    } else |err| {
        try testing.expect(isStableSnapdirError(err));
    }

    // A pull of a non-existent snapshot id from an empty file store must return
    // an error (InvalidId / IoError), never panic, and never partially
    // materialize without signalling failure.
    var store_dir = try tmp.dir.makeOpenPath("emptystore", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    var dest_dir = try tmp.dir.makeOpenPath("dest", .{});
    defer dest_dir.close();
    const dest = try fileUri_destPath(a, dest_dir);
    defer a.free(dest);

    const bad_id: [:0]const u8 = "0000000000000000000000000000000000000000000000000000000000000000";
    if (snapdir.pull(a, bad_id, store_uri, dest, .{})) {
        try testing.expect(false);
    } else |err| {
        try testing.expect(isStableSnapdirError(err));
    }
}

/// destPath returns the plain (non-URI) absolute NUL-terminated filesystem path
/// of `dir` — pull/push take a path, not a file:// URI, for the local tree.
fn fileUri_destPath(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try dir.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "{s}", .{real});
}

// ─── 5. options honoured: exclude changes id; no_follow changes id/manifest ─

test "exclude option changes the snapshot id" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    const base = try snapdir.id(a, root, .{});
    try testing.expect(isHex64(&base));

    // exclude is an extended-regex; dropping a.txt MUST change the id.
    const excluded = try snapdir.id(a, root, .{ .exclude = "a\\.txt" });
    try testing.expect(isHex64(&excluded));
    try testing.expect(!std.mem.eql(u8, &base, &excluded));
}

test "no_follow changes the id and does not dereference the symlink to a File" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    // Add a symlink for the no_follow axis, INSIDE the snapshot tree subdir
    // (buildTree now builds in <tmp>/tree, so the symlink must live there too —
    // a valid `link -> a.txt` relative to the tree). Skip if the FS rejects
    // symlinks (keeps the suite portable).
    var tree_dir = tmp.dir.openDir("tree", .{}) catch return error.SkipZigTest;
    defer tree_dir.close();
    tree_dir.symLink("a.txt", "link", .{}) catch return error.SkipZigTest;

    const follow = try snapdir.id(a, root, .{}); // default follows
    const nofollow = try snapdir.id(a, root, .{ .no_follow = true });
    try testing.expect(isHex64(&follow));
    try testing.expect(isHex64(&nofollow));
    // The binding's id() routes no_follow through manifest()→idFromManifest,
    // so no_follow MUST change the id vs the default follow walk.
    try testing.expect(!std.mem.eql(u8, &follow, &nofollow));

    // id(tree, no_follow) must equal idFromManifest(manifest(tree, no_follow)).
    var mnf = try snapdir.manifest(a, root, .{ .no_follow = true });
    defer mnf.deinit(a);
    const id_mnf = try snapdir.idFromManifest(mnf);
    try testing.expectEqualSlices(u8, &nofollow, &id_mnf);

    // CONTRACT (oracle-backed): snapdir has NO type-L manifest representation —
    // under no_follow a relative symlink is OMITTED, not dereferenced. So we
    // assert the symlink is NOT recorded as a dereferenced File (we do NOT
    // require an 'L' entry to exist).
    for (mnf.entries) |e| {
        if (std.mem.endsWith(u8, e.path, "/link") or std.mem.eql(u8, e.path, "link") or
            std.mem.eql(u8, e.path, "./link"))
        {
            try testing.expect(e.type != .File);
        }
    }
}

// ─── 6. offline round-trip: push → pull restores perms → fetch → diff ───────

test "file:// round-trip: push id == id(tree); pull restores into a pre-existing dir" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    // Robustly clear the tree BEFORE zig's deleteTree runs (it panics on the
    // pulled-dest + object-store layout). Registered after tmp.cleanup → runs
    // first (LIFO); the later FD-closing defers still run before it.
    defer rmrf(a, tmp.dir);
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    // push must not mutate the manifest: the pushed id == the local id(tree).
    const pushed = try snapdir.push(a, root, store_uri, .{});
    try testing.expect(isHex64(&pushed));
    const local = try snapdir.id(a, root, .{});
    try testing.expectEqualSlices(u8, &pushed, &local);

    // fetch the snapshot into the local cache (offline file store).
    const pushed_z = try std.fmt.allocPrintZ(a, "{s}", .{pushed});
    defer a.free(pushed_z);
    try snapdir.fetch(a, pushed_z, store_uri, 0);

    // pull into a PRE-EXISTING, restrictively-permissioned (0o700) dir; the
    // shared permission-restore contract (through the C ABI) means re-id(dest)
    // == pushed. A pull that dropped an entry's mode would re-id differently.
    var dest_dir = try tmp.dir.makeOpenPath("dest", .{});
    dest_dir.chmod(0o700) catch {};
    defer dest_dir.close();
    const dest = try fileUri_destPath(a, dest_dir);
    defer a.free(dest);

    try snapdir.pull(a, pushed_z, store_uri, dest, .{});

    const reid = try snapdir.id(a, dest, .{});
    try testing.expectEqualSlices(u8, &pushed, &reid);
}

test "self-diff over a populated file store yields no change rows" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    _ = try snapdir.push(a, root, store_uri, .{});

    const entries = try snapdir.diff(a, store_uri, store_uri, .{});
    defer {
        for (entries) |e| a.free(e.path);
        a.free(entries);
    }
    for (entries) |e| {
        // status must be a valid DiffStatus and, for a strict self-diff, must
        // never be a change row.
        switch (e.status) {
            .Added, .Deleted, .Modified => try testing.expect(false),
            .Unchanged => {},
        }
    }
}

// ─── 7. no reachable @panic from the public API on error paths ──────────────
//
// Zig has no try/catch-around-@panic — a reachable @panic aborts the test
// binary. So "no panic" is pinned implicitly: every error-path test above
// CATCHES the error (the `else |err|` arms) and the binary keeps running. This
// final test drives several failure inputs back-to-back; reaching the end at
// all proves none of them panicked.

test "error paths are returned, not panicked (binary survives a failure barrage)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    const missing: [:0]const u8 = "/snapdir/no/such/path/barrage-xyz";
    const bad_store: [:0]const u8 = "://://garbage";
    const bad_id: [:0]const u8 = "not-a-valid-hex-id";

    // id() on a missing path.
    _ = snapdir.id(a, missing, .{}) catch {};
    // manifest() on a missing path.
    if (snapdir.manifest(a, missing, .{})) |*m_ok| {
        var m = m_ok.*;
        m.deinit(a);
    } else |_| {}
    // idFromManifest on garbage manifest text (build a bogus Manifest by
    // duplicating raw bytes the impl owns); we go through manifest() of a real
    // tree then mutate is risky, so instead drive an invalid store push.
    _ = snapdir.push(a, missing, bad_store, .{}) catch {};
    // pull of an invalid id from a garbage store.
    snapdir.pull(a, bad_id, bad_store, missing, .{}) catch {};
    // fetch of an invalid id from a garbage store.
    snapdir.fetch(a, bad_id, bad_store, 0) catch {};
    // diff against a garbage store.
    if (snapdir.diff(a, bad_store, bad_store, .{})) |entries| {
        for (entries) |e| a.free(e.path);
        a.free(entries);
    } else |_| {}

    // Reaching here means none of the above panicked — they all returned.
    try testing.expect(true);
}

// ─── 8. error-code EXHAUSTIVENESS — pin the SPECIFIC stable code ─────────────
// (adversary review strengthening, zig-api-tests-review)
//
// The authoring suite pins error PATHS with the generic isStableSnapdirError
// (any member of the set). The C ABI is DETERMINISTIC for two failure classes,
// so we pin the EXACT mapped error here. A regression that mapped an unknown
// scheme to IoError, or a malformed id to InvalidStore, would slip past the
// generic check but FAILS these:
//   - StoreUri::parse rejects any non-{file,s3,gs,b2,ssh,sftp} scheme, an empty
//     scheme, or a missing "://" separator with INVALID_STORE → error.InvalidStore.
//   - pull/fetch validate SnapshotId::from_hex BEFORE the store is parsed, so a
//     non-hex / wrong-length id is INVALID_ID → error.InvalidId, regardless of
//     how bogus the store URI is.

test "unknown / empty / separatorless store scheme maps to error.InvalidStore (exact code)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    // Unknown scheme.
    try testing.expectError(error.InvalidStore, snapdir.push(a, root, "ftp://not-a-store/x", .{}));
    // Empty scheme ("://...") — extract_scheme rejects the empty prefix.
    try testing.expectError(error.InvalidStore, snapdir.push(a, root, "://nope", .{}));
    // No "://" separator at all (a bare path is not a store URI).
    try testing.expectError(error.InvalidStore, snapdir.push(a, root, "not-a-store", .{}));
    // diff surfaces the SAME typed error through a different C entry point.
    try testing.expectError(error.InvalidStore, snapdir.diff(a, "ftp://x", "ftp://y", .{}));
}

test "malformed (non-hex / short) id maps to error.InvalidId before the store is touched" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    // Even paired with a GARBAGE store, the id is parsed first → InvalidId wins.
    const bad_id: [:0]const u8 = "not-a-valid-hex-id";
    const bad_store: [:0]const u8 = "://://garbage";
    const dest: [:0]const u8 = "/snapdir/no/such/dest/zzz";
    try testing.expectError(error.InvalidId, snapdir.fetch(a, bad_id, bad_store, 0));
    try testing.expectError(error.InvalidId, snapdir.pull(a, bad_id, bad_store, dest, .{}));

    // A 63-char (one short of 64) hex string is still a length violation → InvalidId.
    const short_id: [:0]const u8 = "000000000000000000000000000000000000000000000000000000000000000";
    try testing.expectError(error.InvalidId, snapdir.fetch(a, short_id, bad_store, 0));
}

// ─── 9. leak discipline on the ERROR paths (the paramount axis, extended) ────
// (adversary review strengthening, zig-api-tests-review)
//
// Section 1 proves the SUCCESS paths free their allocations. A binding can also
// leak on an EARLY error return — e.g. allocPrintZ'd option strings or a
// partially-built entries slice not freed before the SnapdirError is returned.
// Looping the error paths under std.testing.allocator turns any such per-call
// error-path leak into a hard test failure at teardown.

test "manifest()/id() error returns are leak-free across 40 iterations (paramount)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    const missing: [:0]const u8 = "/snapdir/no/such/path/leak-loop-xyz";
    var i: usize = 0;
    while (i < 40) : (i += 1) {
        // Both error returns carry allocPrintZ'd option strings (exclude /
        // checksum_bin / cache_dir / catalog). If any is not freed before the
        // error propagates, testing.allocator reports it at teardown → FAIL.
        try testing.expectError(error.IoError, snapdir.manifest(a, missing, .{
            .exclude = "x\\.tmp",
            .checksum_bin = "b3sum",
            .catalog = "none",
        }));
        try testing.expectError(error.IoError, snapdir.id(a, missing, .{ .exclude = "x\\.tmp" }));
        // A bogus-scheme push allocates source_id/limit_rate/cache_dir before the
        // store is rejected — those must be freed on the error path too.
        try testing.expectError(error.InvalidStore, snapdir.push(a, missing, "ftp://x", .{
            .limit_rate = "10M",
        }));
    }
}

test "manifest(opts) + diff success paths are leak-free across repeated deinit cycles" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);
    _ = try snapdir.push(a, root, store_uri, .{});

    // manifest() WITH options (exclude + no_follow exercise the allocPrintZ
    // branches) must deinit cleanly every iteration — a per-call leak of raw,
    // entries, any entry.path, or an option string trips testing.allocator.
    var i: usize = 0;
    while (i < 25) : (i += 1) {
        var m = try snapdir.manifest(a, root, .{ .exclude = "nomatch", .no_follow = true });
        defer m.deinit(a);
        try testing.expect(m.raw.len > 0);
        // Every parsed entry path is allocator-owned and non-empty.
        for (m.entries) |e| try testing.expect(e.path.len > 0);

        // diff() in the same loop: the result slice AND each path are freed; a
        // borrowed/double-freed path would fault testing.allocator here.
        const entries = try snapdir.diff(a, store_uri, store_uri, .{});
        defer {
            for (entries) |e| a.free(e.path);
            a.free(entries);
        }
    }
}

// ─── 10. checksum [64]u8 + idFromManifest over an owned Manifest (no double-free) ─
// (adversary review strengthening, zig-api-tests-review)
//
// Pins that idFromManifest reads m.raw without taking ownership (so the caller's
// later deinit is the SOLE free — no double-free) and that a File entry's
// fixed [64]u8 checksum is 64 lowercase-hex, deterministic across re-parses.

test "idFromManifest borrows the Manifest (deinit remains the sole owner; checksum stable)" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildTree(a, tmp.dir);
    defer a.free(root);

    var m = try snapdir.manifest(a, root, .{});
    defer m.deinit(a); // the ONLY free of raw/entries/paths — must not double-free

    // Calling idFromManifest twice must not consume or mutate the Manifest; both
    // ids are byte-identical and the deferred deinit still frees exactly once.
    const id1 = try snapdir.idFromManifest(m);
    const id2 = try snapdir.idFromManifest(m);
    try testing.expect(isHex64(&id1));
    try testing.expectEqualSlices(u8, &id1, &id2);

    // Capture a File entry's checksum, re-parse the SAME tree, and confirm the
    // [64]u8 is deterministic byte-for-byte (content-addressed hashing is stable).
    var first_file_chk: ?[64]u8 = null;
    for (m.entries) |e| {
        if (e.type == .File) {
            try testing.expect(isHex64(&e.checksum));
            first_file_chk = e.checksum;
            break;
        }
    }
    try testing.expect(first_file_chk != null);

    var m2 = try snapdir.manifest(a, root, .{});
    defer m2.deinit(a);
    var matched = false;
    for (m2.entries) |e| {
        if (e.type == .File and std.mem.eql(u8, &e.checksum, &first_file_chk.?)) matched = true;
    }
    try testing.expect(matched); // same file → same 64-hex checksum on re-walk
}
