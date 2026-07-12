// zig_size.zig — black-box spec for the snapdir `size` surface of the
// idiomatic Zig binding (Phase 46, gate zig-size-spec-tests; adversary/opus).
//
// Authored from the SPEC + the Zig binding's PUBLIC surface ONLY
// (bindings/zig/src/snapdir.zig: SnapdirError, PathType, ManifestEntry,
// Manifest, manifest(), push()) and the conventions of the existing
// bindings/zig/test/zig_api.zig. NO visibility into crates/snapdir-ffi/src/
// or crates/snapdir-api/src/ — those Rust sources were NOT read.
//
// Wiring (zig-size-impl): this file is `git mv`'d into the binding's test dir
// and the public surface is imported as a module named "snapdir". These tests
// are EXPECTED TO FAIL TO COMPILE against the current binding because
// `snapdir.size` / `snapdir.manifestSize` / `snapdir.SizeStats` do NOT exist
// yet — the impl gate adds them. Do NOT weaken an assertion to pass.
//
// All tests run under `std.testing.allocator` — its leak detector is the
// PARAMOUNT axis (the Zig analogue of valgrind): a missing Manifest.deinit, an
// un-freed id_z, or a leaked dir-walker trips it and FAILS the test.
//
// SPEC (impl gate MUST satisfy these signatures):
//   pub const SizeStats = struct {
//       logical_bytes: u64, physical_bytes: u64, files: u64, objects: u64 };
//   pub fn size(allocator, store_uri:[:0]const u8, id: ?[:0]const u8)
//              SnapdirError!SizeStats
//       // id == null  ⇒ whole store (deduped);
//       // NUL-terminated 64-hex id ⇒ that snapshot.
//   pub fn manifestSize(allocator, m: Manifest) SnapdirError!SizeStats
//       // SYNC/pure; dedups by the 64-byte checksum; sums over .File entries.
//
// Semantics (objects stored UNCOMPRESSED):
//   logical_bytes  = Σ size over ALL File entries (dups counted)
//   physical_bytes = Σ size over UNIQUE checksums  (== on-disk .objects/ bytes)
//   files          = count of File entries
//   objects        = count of distinct checksums

const std = @import("std");
const snapdir = @import("snapdir");
const testing = std.testing;

// libc setenv — the binding links libc; drive the hermetic/offline env (cache +
// catalog under a temp dir) directly. Mirrors zig_api.zig exactly.
extern "c" fn setenv(name: [*:0]const u8, value: [*:0]const u8, overwrite: c_int) c_int;

// ─── black-box helpers ──────────────────────────────────────────────────────

/// makeOfflineEnv points the cache + catalog at a freshly-created temp dir so
/// no test reaches the network or the user's real cache. The returned tmp dir
/// must be `.cleanup()`d. (Copied verbatim from zig_api.zig conventions.)
fn makeOfflineEnv() !std.testing.TmpDir {
    var tmp = testing.tmpDir(.{});
    errdefer tmp.cleanup();

    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const base = try tmp.dir.realpath(".", &buf);

    var cache_buf: [std.fs.max_path_bytes]u8 = undefined;
    var cat_buf: [std.fs.max_path_bytes]u8 = undefined;
    const cache_z = try std.fmt.bufPrintZ(&cache_buf, "{s}/cache", .{base});
    const cat_z = try std.fmt.bufPrintZ(&cat_buf, "{s}/catalog.db", .{base});

    _ = setenv("SNAPDIR_CACHE_DIR", cache_z.ptr, 1);
    _ = setenv("SNAPDIR_CATALOG_DB_PATH", cat_z.ptr, 1);
    _ = setenv("SNAPDIR_CATALOG", "none", 1);
    return tmp;
}

/// fileUri builds a NUL-terminated "file://<abs>" store URI rooted at `dir`'s
/// realpath. Caller frees. (Mirrors zig_api.zig.)
fn fileUri(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try dir.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "file://{s}", .{real});
}

/// missingUri builds a NUL-terminated "file://<abs>/<non-existent>" store URI
/// whose root directory does NOT exist — the SOURCE-CONFIRMED "missing root ≠
/// empty" case that must return error.StoreError. Caller frees.
fn missingUri(allocator: std.mem.Allocator, tmp: std.testing.TmpDir) ![:0]u8 {
    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const base = try tmp.dir.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "file://{s}/no-such-store-root-xyz", .{base});
}

/// buildSizeTree writes the DETERMINISTIC size fixture (parity with the other 5
/// bindings) under `dir`:
///   seed/a/x.txt   = "hello world\n"      (12 bytes)
///   seed/b/dup.txt = "hello world\n"      (12 bytes — DUPLICATE checksum)
///   seed/a/y.txt   = "unique data here\n" (17 bytes — unique)
/// ⇒ files=3, objects=2, logical_bytes=41, physical_bytes=29.
/// Returns the absolute, NUL-terminated path of the `seed` root. Caller frees.
/// (This is a DIFFERENT tree from zig_api.zig's buildTree — authored fresh.)
fn buildSizeTree(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    var seed = try dir.makeOpenPath("seed", .{});
    defer seed.close();
    try seed.makePath("a");
    try seed.makePath("b");
    try seed.writeFile(.{ .sub_path = "a/x.txt", .data = "hello world\n" }); // 12
    try seed.writeFile(.{ .sub_path = "b/dup.txt", .data = "hello world\n" }); // 12, dup
    try seed.writeFile(.{ .sub_path = "a/y.txt", .data = "unique data here\n" }); // 17

    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try seed.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "{s}", .{real});
}

/// buildDirsOnlyTree writes a tree containing ONLY directories (no files) under
/// `dir`. Its manifest carries no .File entries, so manifestSize MUST report all
/// zeros. Returns the absolute, NUL-terminated root path. Caller frees.
fn buildDirsOnlyTree(allocator: std.mem.Allocator, dir: std.fs.Dir) ![:0]u8 {
    var d = try dir.makeOpenPath("dirsonly", .{});
    defer d.close();
    try d.makePath("x/y");
    try d.makePath("z");

    var buf: [std.fs.max_path_bytes]u8 = undefined;
    const real = try d.realpath(".", &buf);
    return std.fmt.allocPrintZ(allocator, "{s}", .{real});
}

/// objectsBytes walks `<store>/.objects/` and sums the byte size of every
/// regular file (objects are stored UNCOMPRESSED). This is the ACCEPTANCE
/// oracle for physical_bytes. The Walker is `.deinit()`d so the internal path
/// buffers are freed — a leak here would trip testing.allocator. A missing
/// .objects dir yields 0 (an empty/never-pushed store).
fn objectsBytes(allocator: std.mem.Allocator, store_dir: std.fs.Dir) !u64 {
    var objs = store_dir.openDir(".objects", .{ .iterate = true }) catch |e| switch (e) {
        error.FileNotFound => return 0,
        else => return e,
    };
    defer objs.close();

    var total: u64 = 0;
    var walker = try objs.walk(allocator);
    defer walker.deinit(); // frees the walker's heap buffers (leak discipline)
    while (try walker.next()) |entry| {
        if (entry.kind == .file) {
            const st = try entry.dir.statFile(entry.basename);
            total += st.size;
        }
    }
    return total;
}

// ─── 1. manifestSize: pure/sync, dedups by checksum ─────────────────────────

test "manifestSize sums logical/physical bytes and dedups by 64-byte checksum" {
    // clause: files=3, objects=2, logical_bytes=41 (dup COUNTED),
    //         physical_bytes=29 (unique checksums only).
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    // COMPILE-PIN: SizeStats must expose four u64 fields (mirrors C
    // SnapdirSizeStats). If any field is renamed or re-typed this fails to
    // compile — which is CORRECT until the impl lands.
    comptime {
        const L = @TypeOf(@as(snapdir.SizeStats, undefined).logical_bytes);
        if (L != u64) @compileError("SizeStats.logical_bytes must be u64");
        const P = @TypeOf(@as(snapdir.SizeStats, undefined).physical_bytes);
        if (P != u64) @compileError("SizeStats.physical_bytes must be u64");
        const F = @TypeOf(@as(snapdir.SizeStats, undefined).files);
        if (F != u64) @compileError("SizeStats.files must be u64");
        const O = @TypeOf(@as(snapdir.SizeStats, undefined).objects);
        if (O != u64) @compileError("SizeStats.objects must be u64");
    }

    var m = try snapdir.manifest(a, seed, .{});
    defer m.deinit(a); // SOLE free of raw/entries/paths — manifestSize BORROWS

    const s = try snapdir.manifestSize(a, m);
    try testing.expectEqual(@as(u64, 41), s.logical_bytes); // 12 + 12 + 17
    try testing.expectEqual(@as(u64, 29), s.physical_bytes); // 12 + 17 (dedup)
    try testing.expectEqual(@as(u64, 3), s.files); // x.txt, dup.txt, y.txt
    try testing.expectEqual(@as(u64, 2), s.objects); // 2 distinct checksums
}

// ─── 2. size(store, id) of a pushed snapshot == manifestSize (all 4 fields) ──

test "size(store, NUL-terminated id) equals the pure manifestSize figure" {
    // clause: push the seed, NUL-terminate the returned [64]u8 id, and query
    // that single snapshot; every field must match manifestSize.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    const pushed = try snapdir.push(a, seed, store_uri, .{}); // [64]u8 value

    // NUL-terminate the fixed [64]u8 id into a [:0]u8 for the `?[:0]const u8`
    // parameter. allocPrintZ appends the sentinel; freed via defer (leak axis).
    const id_z = try std.fmt.allocPrintZ(a, "{s}", .{&pushed});
    defer a.free(id_z);

    // Oracle: the pure manifestSize of the same seed.
    var m = try snapdir.manifest(a, seed, .{});
    defer m.deinit(a);
    const oracle = try snapdir.manifestSize(a, m);

    const s = try snapdir.size(a, store_uri, id_z);
    try testing.expectEqual(oracle.logical_bytes, s.logical_bytes);
    try testing.expectEqual(oracle.physical_bytes, s.physical_bytes);
    try testing.expectEqual(oracle.files, s.files);
    try testing.expectEqual(oracle.objects, s.objects);
    // Absolute pins too (independent of the oracle).
    try testing.expectEqual(@as(u64, 41), s.logical_bytes);
    try testing.expectEqual(@as(u64, 29), s.physical_bytes);
}

// ─── 3. size(store, null): whole store (single snapshot) == the one figure ───

test "size(store, null) over a one-snapshot store equals the single figure" {
    // clause: whole-store (deduped) size of a store holding exactly one
    // snapshot equals that snapshot's figure; physical_bytes==29 absolutely.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    _ = try snapdir.push(a, seed, store_uri, .{});

    const s = try snapdir.size(a, store_uri, null);
    try testing.expectEqual(@as(u64, 41), s.logical_bytes);
    try testing.expectEqual(@as(u64, 29), s.physical_bytes); // absolutely 29
    try testing.expectEqual(@as(u64, 3), s.files);
    try testing.expectEqual(@as(u64, 2), s.objects);
}

// ─── 4. ACCEPTANCE: physical_bytes == on-disk .objects/ byte total == 29 ─────

test "ACCEPTANCE: whole-store physical_bytes equals the on-disk .objects/ sum" {
    // clause: the CORE acceptance bar — size(...).physical_bytes MUST equal the
    // real byte total of <store>/.objects/ (uncompressed) and equal 29.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    _ = try snapdir.push(a, seed, store_uri, .{});

    const on_disk = try objectsBytes(a, store_dir); // walk .objects/, sum files
    const s = try snapdir.size(a, store_uri, null);

    try testing.expectEqual(on_disk, s.physical_bytes); // reported == on-disk
    try testing.expectEqual(@as(u64, 29), on_disk); // and both == 29
    try testing.expectEqual(@as(u64, 29), s.physical_bytes);
}

// ─── 5. strict inequalities: dedup MUST bite for this fixture ────────────────

test "strict: logical_bytes > physical_bytes AND objects < files (dedup bites)" {
    // clause: the fixture contains a genuine duplicate, so the deduped physical
    // total is strictly less than the logical total and distinct objects are
    // strictly fewer than files. A no-op dedup would fail this.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    var m = try snapdir.manifest(a, seed, .{});
    defer m.deinit(a);
    const s = try snapdir.manifestSize(a, m);

    try testing.expect(s.logical_bytes > s.physical_bytes); // 41 > 29
    try testing.expect(s.objects < s.files); // 2 < 3
}

// ─── 6. STORE-ERROR contract (SOURCE-CONFIRMED, three cases) ─────────────────

test "store-error contract: missing root errors; empty store is zero; bad scheme is InvalidStore" {
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    // (a) MISSING-root file:// store (dir does NOT exist) ⇒ error.StoreError.
    //     THE important case: a missing root is NOT an empty store.
    const missing = try missingUri(a, tmp);
    defer a.free(missing);
    try testing.expectError(error.StoreError, snapdir.size(a, missing, null));

    // (b) EXISTING store dir with NO manifests (push nothing) ⇒ all-ZERO
    //     SizeStats, NO error.
    var empty_dir = try tmp.dir.makeOpenPath("empty_store", .{});
    defer empty_dir.close();
    const empty_uri = try fileUri(a, empty_dir);
    defer a.free(empty_uri);
    const z = try snapdir.size(a, empty_uri, null);
    try testing.expectEqual(@as(u64, 0), z.logical_bytes);
    try testing.expectEqual(@as(u64, 0), z.physical_bytes);
    try testing.expectEqual(@as(u64, 0), z.files);
    try testing.expectEqual(@as(u64, 0), z.objects);

    // (c) malformed / unknown scheme ⇒ error.InvalidStore.
    try testing.expectError(error.InvalidStore, snapdir.size(a, "bogus://nope", null));
}

// ─── 7. degenerate: manifestSize of a dirs-only Manifest ⇒ all zeros ─────────

test "manifestSize of a directories-only manifest is all zeros" {
    // clause: with no .File entries there is nothing to sum or dedup — every
    // SizeStats field is zero (not an error).
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const root = try buildDirsOnlyTree(a, tmp.dir);
    defer a.free(root);

    var m = try snapdir.manifest(a, root, .{});
    defer m.deinit(a);

    // Sanity: this manifest really has no File entries (only Directory rows).
    for (m.entries) |e| try testing.expect(e.type != .File);

    const s = try snapdir.manifestSize(a, m);
    try testing.expectEqual(@as(u64, 0), s.logical_bytes);
    try testing.expectEqual(@as(u64, 0), s.physical_bytes);
    try testing.expectEqual(@as(u64, 0), s.files);
    try testing.expectEqual(@as(u64, 0), s.objects);
}

// ─── 8. LEAK DISCIPLINE (paramount) — repeated cycles under testing.allocator ─

test "size + manifestSize are leak-free across repeated cycles (paramount)" {
    // clause: loop the whole-store size(), the by-id size() (freeing id_z each
    // pass), manifestSize() (deinit each Manifest), and the .objects/ walker so
    // any per-call leak of the dedup set, an option string, or a walk buffer is
    // reported by testing.allocator at teardown → FAIL.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();
    const seed = try buildSizeTree(a, tmp.dir);
    defer a.free(seed);

    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    const pushed = try snapdir.push(a, seed, store_uri, .{});

    var i: usize = 0;
    while (i < 25) : (i += 1) {
        // manifestSize path: Manifest MUST deinit cleanly every pass.
        var m = try snapdir.manifest(a, seed, .{});
        defer m.deinit(a);
        const ms = try snapdir.manifestSize(a, m);
        try testing.expectEqual(@as(u64, 29), ms.physical_bytes);

        // by-id size path: id_z is allocPrintZ'd and freed every pass.
        const id_z = try std.fmt.allocPrintZ(a, "{s}", .{&pushed});
        defer a.free(id_z);
        const sid = try snapdir.size(a, store_uri, id_z);
        try testing.expectEqual(@as(u64, 41), sid.logical_bytes);

        // whole-store size path.
        const sall = try snapdir.size(a, store_uri, null);
        try testing.expectEqual(@as(u64, 2), sall.objects);

        // .objects/ walker path (its Walker.deinit frees internal buffers).
        const disk = try objectsBytes(a, store_dir);
        try testing.expectEqual(@as(u64, 29), disk);
    }
}

// ─── 9. HARDENING (adversary review): cross-snapshot whole-store dedup ────────

test "cross-snapshot dedup: whole-store dedups a shared object across snapshots" {
    // clause: push TWO distinct snapshots into ONE store. Snapshot B shares
    // exactly ONE object with A ("hello world\n") and adds one brand-new object.
    // The whole-store (deduped) size must therefore report:
    //   files          = refA.files + refB.files          (files are NOT deduped)
    //   logical_bytes  = refA.logical_bytes + refB.logical_bytes (dups counted)
    //   physical_bytes < refA.physical_bytes + refB.physical_bytes (shared object
    //                    stored ONCE ⇒ STRICTLY less)
    //   objects        < refA.objects + refB.objects       (STRICTLY fewer)
    // Every reference value is COMPUTED from the local manifests — no hardcoded
    // byte counts — so the test tracks the fixtures, not a magic number.
    const a = testing.allocator;
    var tmp = try makeOfflineEnv();
    defer tmp.cleanup();

    // Seed A: the canonical size fixture (files=3, objects=2).
    const seedA = try buildSizeTree(a, tmp.dir);
    defer a.free(seedA);

    // Seed B: a fresh subdir with two files — one that SHARES A's 12-byte
    // "hello world\n" object, and one with content unique to the whole store.
    var bdir = try tmp.dir.makeOpenPath("seedB", .{});
    defer bdir.close();
    try bdir.makePath("sub");
    try bdir.writeFile(.{ .sub_path = "sub/shared.txt", .data = "hello world\n" }); // dup of A
    try bdir.writeFile(.{ .sub_path = "sub/fresh.txt", .data = "brand new content!\n" }); // unique
    var bbuf: [std.fs.max_path_bytes]u8 = undefined;
    const breal = try bdir.realpath(".", &bbuf);
    const seedB = try std.fmt.allocPrintZ(a, "{s}", .{breal});
    defer a.free(seedB);

    // One shared store for BOTH snapshots.
    var store_dir = try tmp.dir.makeOpenPath("store", .{});
    defer store_dir.close();
    const store_uri = try fileUri(a, store_dir);
    defer a.free(store_uri);

    const id_a = try snapdir.push(a, seedA, store_uri, .{}); // [64]u8
    const id_b = try snapdir.push(a, seedB, store_uri, .{}); // [64]u8

    // Distinct snapshots (different content ⇒ different manifest checksum).
    try testing.expect(!std.mem.eql(u8, &id_a, &id_b)); // A and B are distinct ids

    // NUL-terminate each id for the `?[:0]const u8` size() parameter.
    const ida_z = try std.fmt.allocPrintZ(a, "{s}", .{&id_a});
    defer a.free(ida_z);
    const idb_z = try std.fmt.allocPrintZ(a, "{s}", .{&id_b});
    defer a.free(idb_z);

    // COMPUTED reference figures from the local manifests (no hardcoded bytes).
    var mA = try snapdir.manifest(a, seedA, .{});
    defer mA.deinit(a);
    const refA = try snapdir.manifestSize(a, mA);
    var mB = try snapdir.manifest(a, seedB, .{});
    defer mB.deinit(a);
    const refB = try snapdir.manifestSize(a, mB);

    // Per-id size() must equal each snapshot's own manifestSize (all 4 fields).
    const sa = try snapdir.size(a, store_uri, ida_z);
    try testing.expectEqual(refA.logical_bytes, sa.logical_bytes); // A: logical matches
    try testing.expectEqual(refA.physical_bytes, sa.physical_bytes); // A: physical matches
    try testing.expectEqual(refA.files, sa.files); // A: files match
    try testing.expectEqual(refA.objects, sa.objects); // A: objects match
    const sb = try snapdir.size(a, store_uri, idb_z);
    try testing.expectEqual(refB.logical_bytes, sb.logical_bytes); // B: logical matches
    try testing.expectEqual(refB.physical_bytes, sb.physical_bytes); // B: physical matches
    try testing.expectEqual(refB.files, sb.files); // B: files match
    try testing.expectEqual(refB.objects, sb.objects); // B: objects match

    // Sanity: B has NO internal duplicate (its two files differ), so within B
    // alone logical == physical — the dedup below is purely CROSS-snapshot.
    try testing.expectEqual(refB.logical_bytes, refB.physical_bytes); // B: no internal dup
    try testing.expectEqual(@as(u64, 2), refB.files); // shared.txt + fresh.txt
    try testing.expect(refA.objects >= 2); // A carries the shared 12-byte object

    // Whole-store (deduped) figures.
    const whole = try snapdir.size(a, store_uri, null);
    // Files & logical bytes are ADDITIVE across snapshots (never deduped).
    try testing.expectEqual(refA.files + refB.files, whole.files); // files sum
    try testing.expectEqual(refA.logical_bytes + refB.logical_bytes, whole.logical_bytes); // logical sum
    // Physical bytes & object count are STRICTLY less than the naive sum,
    // because the "hello world\n" object is stored exactly once store-wide.
    try testing.expect(whole.physical_bytes < refA.physical_bytes + refB.physical_bytes); // shared object stored once
    try testing.expect(whole.objects < refA.objects + refB.objects); // fewer distinct objects store-wide

    // Whole-store physical_bytes must equal the real on-disk .objects/ total.
    const on_disk = try objectsBytes(a, store_dir);
    try testing.expectEqual(on_disk, whole.physical_bytes); // reported == on-disk

    // Determinism: a second whole-store query returns identical figures.
    const whole2 = try snapdir.size(a, store_uri, null);
    try testing.expectEqual(whole.logical_bytes, whole2.logical_bytes); // stable logical
    try testing.expectEqual(whole.physical_bytes, whole2.physical_bytes); // stable physical
    try testing.expectEqual(whole.files, whole2.files); // stable files
    try testing.expectEqual(whole.objects, whole2.objects); // stable objects
}
