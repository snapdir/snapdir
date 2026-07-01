// snapdir.zig — idiomatic Zig public surface over the snapdir-ffi C ABI.
//
// The C ABI layer lives in src/c.zig (imported here only, never by callers).
// Every allocating function takes `std.mem.Allocator` as its first argument,
// enabling arena use and `std.testing.allocator` leak detection in tests.
//
// Memory contract:
//   - C strings returned by char*-returning fns are freed with
//     c.snapdir_string_free() — always via `defer`.
//   - SnapdirError* values are freed with c.snapdir_error_free() via defer.
//   - snapdir_version() returns a static C string: NEVER freed.
//   - snapdir_error_code() / snapdir_error_message() return borrowed pointers
//     valid only for the lifetime of the SnapdirError*: copy before free.
//
// Error mapping (C code → Zig error):
//   IO_ERROR       → IoError
//   HASH_MISMATCH  → HashMismatch
//   STORE_ERROR    → StoreError
//   IN_FLUX        → InFlux
//   CATALOG_ERROR  → CatalogError
//   INVALID_ID     → InvalidId
//   INVALID_STORE  → InvalidStore
//   CONFLICT       → Conflict
//   INTERNAL / *   → OutOfMemory (unexpected; treated as a fatal internal fault)
//
// Design notes:
//   - `checksum` fields are `[64]u8` (fixed array, hex-ASCII) — never heap.
//   - `Manifest` owns its string data; call `Manifest.deinit(allocator)` to free.
//   - No reachable `@panic` from the public API surface (-Doptimize=ReleaseSafe safe).
//   - The public `version()` fn copies the static C string into a Zig-owned slice
//     so callers can use it without lifetime concerns.  The caller frees it.

const std = @import("std");
const c = @import("c.zig").c;

// ─── Error set ────────────────────────────────────────────────────────────────

/// The complete Zig error set for snapdir operations.
/// Codes map 1-to-1 to the 8 stable C ABI error codes + the catch-all.
pub const SnapdirError = error{
    IoError,
    HashMismatch,
    StoreError,
    InFlux,
    CatalogError,
    InvalidId,
    InvalidStore,
    Conflict,
    OutOfMemory,
};

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// init calls snapdir_init() once.  The underlying function is idempotent so
/// calling it before every public function is safe and cheap.
fn init() void {
    c.snapdir_init();
}

/// mapError reads the error code from a C SnapdirError*, frees it, and returns
/// the corresponding Zig error.  If err_ptr is null it returns OutOfMemory as a
/// sentinel for "unexpected null error" (should not occur under the ABI contract).
fn mapError(err_ptr: ?*c.SnapdirError) SnapdirError {
    const p = err_ptr orelse return error.OutOfMemory;
    defer c.snapdir_error_free(p);
    const code_ptr = c.snapdir_error_code(p);
    if (code_ptr == null) return error.OutOfMemory;
    const code = std.mem.span(code_ptr);
    if (std.mem.eql(u8, code, "IO_ERROR")) return error.IoError;
    if (std.mem.eql(u8, code, "HASH_MISMATCH")) return error.HashMismatch;
    if (std.mem.eql(u8, code, "STORE_ERROR")) return error.StoreError;
    if (std.mem.eql(u8, code, "IN_FLUX")) return error.InFlux;
    if (std.mem.eql(u8, code, "CATALOG_ERROR")) return error.CatalogError;
    if (std.mem.eql(u8, code, "INVALID_ID")) return error.InvalidId;
    if (std.mem.eql(u8, code, "INVALID_STORE")) return error.InvalidStore;
    if (std.mem.eql(u8, code, "CONFLICT")) return error.Conflict;
    // INTERNAL or any unrecognised code
    return error.OutOfMemory;
}

// ─── PathType ─────────────────────────────────────────────────────────────────

/// PathType identifies the kind of a manifest entry.
/// Values match the single-character TYPE field in the manifest text.
pub const PathType = enum(u8) {
    File      = 'F',
    Directory = 'D',
    Symlink   = 'L',
};

// ─── DiffStatus ───────────────────────────────────────────────────────────────

/// DiffStatus is the change indicator for a diff entry.
pub const DiffStatus = enum(u8) {
    Added     = 'A',
    Deleted   = 'D',
    Modified  = 'M',
    Unchanged = '=',
};

// ─── ManifestEntry ────────────────────────────────────────────────────────────

/// ManifestEntry is one parsed line from a snapshot manifest.
/// `checksum` is a fixed 64-char BLAKE3 hex array (zero-filled for directories).
/// `path` is allocator-owned and freed by `Manifest.deinit`.
pub const ManifestEntry = struct {
    type:     PathType,
    perm:     u32,      // POSIX mode bits (octal, e.g. 0o644)
    checksum: [64]u8,   // 64-char BLAKE3 hex; zeroed for directories
    size:     u64,      // byte count
    path:     []const u8,
};

// ─── Manifest ─────────────────────────────────────────────────────────────────

/// Manifest holds a parsed snapshot manifest.
/// Call `deinit` to free all allocator-owned memory (raw text + entry paths).
pub const Manifest = struct {
    /// The full manifest text returned by the C library. NUL-terminated so
    /// `raw.ptr` can be passed straight to `snapdir_id_from_manifest_text`
    /// (a C `const char*`) without reading past the buffer.
    raw:     [:0]u8,
    /// Parsed manifest entries (paths owned by the allocator).
    entries: []ManifestEntry,

    /// Frees all allocator-owned memory held by this Manifest.
    pub fn deinit(self: *Manifest, allocator: std.mem.Allocator) void {
        for (self.entries) |entry| {
            allocator.free(entry.path);
        }
        allocator.free(self.entries);
        allocator.free(self.raw);
        self.* = undefined;
    }
};

// ─── DiffEntry ────────────────────────────────────────────────────────────────

/// DiffEntry is one entry from a store diff result.
/// `path` is allocator-owned; free with the allocator when done.
pub const DiffEntry = struct {
    status: DiffStatus,
    path:   []const u8,
};

// ─── ManifestOptions ─────────────────────────────────────────────────────────

/// ManifestOptions controls the directory walk for `manifest` and `id`.
/// All fields default to the C ABI NULL/0/false semantics.
pub const ManifestOptions = struct {
    /// Extended-regex exclusion pattern (null = no exclusion).
    exclude:      ?[]const u8 = null,
    /// Parallel hashing worker count (0 = auto/CPU-count default).
    walk_jobs:    u32         = 0,
    /// Emit absolute paths instead of ./-relative paths.
    absolute:     bool        = false,
    /// Do not follow symbolic links.
    no_follow:    bool        = false,
    /// Checksum algorithm (null = "b3sum" default; "md5sum" or "sha256sum").
    checksum_bin: ?[]const u8 = null,
    /// Override the local object-cache directory (null = default).
    cache_dir:    ?[]const u8 = null,
    /// Catalog adapter selector (null = adapter default; "none" = suppress).
    catalog:      ?[]const u8 = null,
};

// ─── PushOptions ─────────────────────────────────────────────────────────────

/// PushOptions controls optional parameters for `push`.
pub const PushOptions = struct {
    /// Push a previously-staged snapshot by id instead of staging from path.
    source_id:   ?[]const u8 = null,
    /// Max concurrent transfers (0 = default).
    jobs:        u32         = 0,
    /// Bandwidth cap string (e.g. "10M"; null = unlimited).
    limit_rate:  ?[]const u8 = null,
    /// Max retry attempts per object (0 = default of 5).
    max_retries: u32         = 0,
    /// Override local cache directory (null = default).
    cache_dir:   ?[]const u8 = null,
};

// ─── PullOptions ─────────────────────────────────────────────────────────────

/// PullOptions controls optional parameters for `pull`.
pub const PullOptions = struct {
    /// Delete destination files absent from the snapshot.
    delete_extra: bool = false,
    /// Max concurrent transfers (0 = default).
    jobs:         u32  = 0,
};

// ─── DiffOptions ─────────────────────────────────────────────────────────────

/// DiffOptions controls optional parameters for `diff`.
pub const DiffOptions = struct {
    /// Optional 64-hex snapshot id (null = A→B cross-store diff).
    snapshot_id:      ?[]const u8 = null,
    /// Include unchanged entries in the output.
    include_unchanged: bool        = false,
    /// Conflict policy: "error" or "last-wins" (null = "error").
    on_conflict:      ?[]const u8 = null,
};

// ─── Internal: manifest text parser ──────────────────────────────────────────

/// parseManifestText splits manifest text into ManifestEntry values.
/// Format: TYPE PERM CHECKSUM SIZE PATH  (one entry per line).
/// Lines starting with '#' or blank lines are skipped.
/// All path strings are duplicated with `allocator`.
fn parseManifestText(
    allocator: std.mem.Allocator,
    text: []const u8,
) ![]ManifestEntry {
    var entries = std.ArrayList(ManifestEntry).init(allocator);
    errdefer {
        for (entries.items) |e| allocator.free(e.path);
        entries.deinit();
    }

    var lines = std.mem.splitScalar(u8, text, '\n');
    while (lines.next()) |raw_line| {
        // Strip trailing CR for CRLF inputs.
        const line = std.mem.trimRight(u8, raw_line, "\r");
        if (line.len == 0 or line[0] == '#') continue;

        // Split into at most 5 tokens: TYPE PERM CHECKSUM SIZE PATH
        var it = std.mem.splitScalar(u8, line, ' ');
        const type_str = it.next() orelse continue;
        const perm_str = it.next() orelse continue;
        const chk_str  = it.next() orelse continue;
        const size_str = it.next() orelse continue;
        const path_str = it.rest();
        if (path_str.len == 0 or type_str.len == 0) continue;

        const pt: PathType = switch (type_str[0]) {
            'F' => .File,
            'D' => .Directory,
            'L' => .Symlink,
            else => continue,
        };

        const perm = std.fmt.parseInt(u32, perm_str, 8) catch continue;
        const size = std.fmt.parseInt(u64, size_str, 10) catch continue;

        var checksum: [64]u8 = [_]u8{0} ** 64;
        if (chk_str.len <= 64) {
            @memcpy(checksum[0..chk_str.len], chk_str);
        }

        const path_owned = try allocator.dupe(u8, path_str);
        try entries.append(.{
            .type     = pt,
            .perm     = perm,
            .checksum = checksum,
            .size     = size,
            .path     = path_owned,
        });
    }

    return entries.toOwnedSlice();
}

// ─── Internal: null-terminated helper ────────────────────────────────────────

/// toSentinel converts an optional Zig slice to a null-terminated C pointer.
/// Returns null when the slice is null.
/// IMPORTANT: the caller must ensure the underlying slice is null-terminated
/// or use `std.fmt.allocPrintZ` / `std.mem.dupeZ` before calling.
/// In the public API we rely on the fact that all string literals in Zig are
/// already null-terminated when expressed as string literals.
fn optCstr(s: ?[]const u8) ?[*:0]const u8 {
    const sl = s orelse return null;
    // Cast: the slice must already be null-terminated (caller's responsibility).
    // Functions below use allocPrintZ or string literals to ensure this.
    return @ptrCast(sl.ptr);
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// version returns the snapdir-api version string (e.g. "1.10.0") as a
/// caller-owned slice.  The caller must free with `allocator.free(result)`.
///
/// The underlying C string has static lifetime; this fn copies it so callers
/// do not need to reason about the static lifetime.
pub fn version(allocator: std.mem.Allocator) ![]u8 {
    init();
    const ptr = c.snapdir_version();
    if (ptr == null) return allocator.dupe(u8, "");
    const sv = std.mem.span(ptr);
    return allocator.dupe(u8, sv);
}

/// manifest walks `path` and returns the parsed directory manifest.
/// The returned Manifest owns its memory; call `result.deinit(allocator)` when done.
pub fn manifest(
    allocator: std.mem.Allocator,
    path: [:0]const u8,
    opts: ManifestOptions,
) !Manifest {
    init();

    // Build null-terminated versions of optional strings.
    const excl = if (opts.exclude) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (excl) |e| allocator.free(e);
    const chk  = if (opts.checksum_bin) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (chk) |ch| allocator.free(ch);
    const cd   = if (opts.cache_dir) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (cd) |d| allocator.free(d);
    const cat  = if (opts.catalog) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (cat) |ct| allocator.free(ct);

    var err_ptr: ?*c.SnapdirError = null;
    const raw_ptr = c.snapdir_manifest(
        path.ptr,
        if (excl) |e| e.ptr else null,
        opts.walk_jobs,
        opts.absolute,
        opts.no_follow,
        if (chk) |ch| ch.ptr else null,
        if (cd) |d| d.ptr else null,
        if (cat) |ct| ct.ptr else null,
        &err_ptr,
    );
    if (raw_ptr == null) {
        return mapError(err_ptr);
    }
    defer c.snapdir_string_free(raw_ptr);

    const raw_text = std.mem.span(raw_ptr);
    const raw_owned = try allocator.dupeZ(u8, raw_text); // NUL-terminated for idFromManifest
    errdefer allocator.free(raw_owned);

    const entries = try parseManifestText(allocator, raw_owned);
    return Manifest{ .raw = raw_owned, .entries = entries };
}

/// id computes the 64-char lowercase hex BLAKE3 snapshot id for the directory
/// at `path`.  Returns a caller-owned 64-byte array (no allocation needed).
///
/// Note: ManifestOptions.absolute and .no_follow are not exposed by
/// snapdir_id(); when either is set the id is derived via manifest().
pub fn id(
    allocator: std.mem.Allocator,
    path: [:0]const u8,
    opts: ManifestOptions,
) ![64]u8 {
    init();

    if (opts.absolute or opts.no_follow) {
        // Route through manifest → id_from_manifest (matches C++/Go precedent).
        var m = try manifest(allocator, path, opts);
        defer m.deinit(allocator);
        return idFromManifest(m);
    }

    const excl = if (opts.exclude) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (excl) |e| allocator.free(e);
    const cd   = if (opts.cache_dir) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (cd) |d| allocator.free(d);

    var err_ptr: ?*c.SnapdirError = null;
    const raw_ptr = c.snapdir_id(
        path.ptr,
        if (excl) |e| e.ptr else null,
        opts.walk_jobs,
        if (cd) |d| d.ptr else null,
        &err_ptr,
    );
    if (raw_ptr == null) {
        return mapError(err_ptr);
    }
    defer c.snapdir_string_free(raw_ptr);

    const sl = std.mem.span(raw_ptr);
    var result: [64]u8 = undefined;
    if (sl.len >= 64) {
        @memcpy(&result, sl[0..64]);
    } else {
        @memcpy(result[0..sl.len], sl);
        @memset(result[sl.len..], 0);
    }
    return result;
}

/// idFromManifest computes the snapshot id from an already-computed Manifest.
/// This is a pure synchronous operation (no I/O).
pub fn idFromManifest(m: Manifest) ![64]u8 {
    init();
    var err_ptr: ?*c.SnapdirError = null;
    const raw_ptr = c.snapdir_id_from_manifest_text(
        m.raw.ptr,
        &err_ptr,
    );
    if (raw_ptr == null) {
        return mapError(err_ptr);
    }
    defer c.snapdir_string_free(raw_ptr);

    const sl = std.mem.span(raw_ptr);
    var result: [64]u8 = undefined;
    if (sl.len >= 64) {
        @memcpy(&result, sl[0..64]);
    } else {
        @memcpy(result[0..sl.len], sl);
        @memset(result[sl.len..], 0);
    }
    return result;
}

/// push stages the directory at `path` and pushes it to `store_uri`.
/// Returns the 64-char hex snapshot id as a fixed `[64]u8` array.
pub fn push(
    allocator: std.mem.Allocator,
    path: [:0]const u8,
    store_uri: [:0]const u8,
    opts: PushOptions,
) ![64]u8 {
    init();

    const sid  = if (opts.source_id) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (sid) |s| allocator.free(s);
    const lr   = if (opts.limit_rate) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (lr) |s| allocator.free(s);
    const cd   = if (opts.cache_dir) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (cd) |s| allocator.free(s);

    var err_ptr: ?*c.SnapdirError = null;
    const raw_ptr = c.snapdir_push_blocking(
        path.ptr,
        if (sid) |s| s.ptr else null,
        store_uri.ptr,
        opts.jobs,
        if (lr) |s| s.ptr else null,
        opts.max_retries,
        if (cd) |s| s.ptr else null,
        &err_ptr,
    );
    if (raw_ptr == null) {
        return mapError(err_ptr);
    }
    defer c.snapdir_string_free(raw_ptr);

    const sl = std.mem.span(raw_ptr);
    var result: [64]u8 = undefined;
    if (sl.len >= 64) {
        @memcpy(&result, sl[0..64]);
    } else {
        @memcpy(result[0..sl.len], sl);
        @memset(result[sl.len..], 0);
    }
    return result;
}

/// pull fetches a snapshot from `store_uri` and materializes it into `dest_path`.
pub fn pull(
    allocator: std.mem.Allocator,
    snapshot_id: [:0]const u8,
    store_uri: [:0]const u8,
    dest_path: [:0]const u8,
    opts: PullOptions,
) !void {
    _ = allocator; // no heap use in this path
    init();
    var err_ptr: ?*c.SnapdirError = null;
    const rc = c.snapdir_pull_blocking(
        snapshot_id.ptr,
        store_uri.ptr,
        dest_path.ptr,
        opts.delete_extra,
        opts.jobs,
        &err_ptr,
    );
    if (rc != 0) {
        return mapError(err_ptr);
    }
}

/// fetch downloads a snapshot from `store_uri` into the local cache.
pub fn fetch(
    allocator: std.mem.Allocator,
    snapshot_id: [:0]const u8,
    store_uri: [:0]const u8,
    jobs: u32,
) !void {
    _ = allocator;
    init();
    var err_ptr: ?*c.SnapdirError = null;
    const rc = c.snapdir_fetch_blocking(
        snapshot_id.ptr,
        store_uri.ptr,
        jobs,
        &err_ptr,
    );
    if (rc != 0) {
        return mapError(err_ptr);
    }
}

/// diff diffs two stores and returns allocator-owned []DiffEntry.
/// Caller must free each entry's `path` and the slice itself.
pub fn diff(
    allocator: std.mem.Allocator,
    from_uri: [:0]const u8,
    to_uri: [:0]const u8,
    opts: DiffOptions,
) ![]DiffEntry {
    init();

    const snap_id = if (opts.snapshot_id) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (snap_id) |s| allocator.free(s);
    const oc      = if (opts.on_conflict) |s| (try std.fmt.allocPrintZ(allocator, "{s}", .{s})) else null;
    defer if (oc) |s| allocator.free(s);

    // Build NULL-terminated arrays for from_uris and to_uris.
    var from_arr = [2]?[*:0]const u8{ from_uri.ptr, null };
    var to_arr   = [2]?[*:0]const u8{ to_uri.ptr,   null };

    var err_ptr: ?*c.SnapdirError = null;
    const json_ptr = c.snapdir_diff_json(
        @ptrCast(@alignCast(&from_arr)),
        @ptrCast(@alignCast(&to_arr)),
        if (snap_id) |s| s.ptr else null,
        opts.include_unchanged,
        if (oc) |s| s.ptr else null,
        &err_ptr,
    );
    if (json_ptr == null) {
        return mapError(err_ptr);
    }
    defer c.snapdir_string_free(json_ptr);

    const json_text = std.mem.span(json_ptr);
    return parseDiffJson(allocator, json_text);
}

// ─── Internal: minimal diff JSON parser ──────────────────────────────────────

/// parseDiffJson parses the JSON array returned by snapdir_diff_json.
/// Shape: [{"status":"A","path":"./x"}, ...]
/// Returns allocator-owned []DiffEntry; caller frees each path and the slice.
fn parseDiffJson(allocator: std.mem.Allocator, json: []const u8) ![]DiffEntry {
    var entries = std.ArrayList(DiffEntry).init(allocator);
    errdefer {
        for (entries.items) |e| allocator.free(e.path);
        entries.deinit();
    }

    var pos: usize = 0;
    while (pos < json.len) {
        // Find next object opening brace.
        const obj_open = std.mem.indexOfScalarPos(u8, json, pos, '{') orelse break;

        // Find the matching closing brace (track depth, skip strings).
        var depth: usize = 0;
        var obj_close: ?usize = null;
        var i = obj_open;
        while (i < json.len) : (i += 1) {
            switch (json[i]) {
                '\\' => i += 1, // skip escaped char
                '"' => {
                    // Skip string body.
                    i += 1;
                    while (i < json.len) : (i += 1) {
                        if (json[i] == '\\') {
                            i += 1;
                        } else if (json[i] == '"') {
                            break;
                        }
                    }
                },
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if (depth == 0) {
                        obj_close = i;
                        break;
                    }
                },
                else => {},
            }
        }
        const close = obj_close orelse break;
        const obj = json[obj_open .. close + 1];

        // Extract "status":"X" — value is exactly 1 char.
        var status_char: u8 = 0;
        if (std.mem.indexOf(u8, obj, "\"status\":\"")) |st| {
            const val_pos = st + 10; // skip past "status":"
            if (val_pos < obj.len) status_char = obj[val_pos];
        }

        // Extract "path":"Y" with basic escape handling.
        var path_str: ?[]const u8 = null;
        if (std.mem.indexOf(u8, obj, "\"path\":\"")) |pa| {
            const start = pa + 8; // skip past "path":"
            if (start < obj.len) {
                var end = start;
                var escaped = false;
                while (end < obj.len) : (end += 1) {
                    if (escaped) {
                        escaped = false;
                    } else if (obj[end] == '\\') {
                        escaped = true;
                    } else if (obj[end] == '"') {
                        break;
                    }
                }
                if (end < obj.len) {
                    path_str = obj[start..end];
                }
            }
        }

        const ps = path_str orelse { pos = close + 1; continue; };
        const status: DiffStatus = switch (status_char) {
            'A' => .Added,
            'D' => .Deleted,
            'M' => .Modified,
            '=' => .Unchanged,
            else => { pos = close + 1; continue; },
        };

        const path_owned = try allocator.dupe(u8, ps);
        try entries.append(.{ .status = status, .path = path_owned });
        pos = close + 1;
    }

    return entries.toOwnedSlice();
}
