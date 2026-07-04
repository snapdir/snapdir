# snapdir (Zig)

Idiomatic **Zig bindings** for [snapdir](https://snapdir.org) — a content-addressed
directory snapshot tool that walks a directory tree, hashes every file with
BLAKE3, and produces a stable snapshot ID you can push to and pull from object
stores (file, S3, GCS, Backblaze B2, SSH).

The binding is a thin `@cImport` wrapper (`src/snapdir.zig`) over the snapdir
**C ABI** (`snapdir.h`), which itself wraps the canonical `snapdir-api` core — so
manifests and snapshot IDs are **bit-identical** to the `snapdir` CLI and to the
Node, Python, Go, and C++ bindings.

- Minimum Zig: **0.13.0**.
- **Allocator injection** — every allocating function takes a `std.mem.Allocator`
  first (use an arena or `std.testing.allocator` for leak detection).
- `checksum` is a fixed `[64]u8` (BLAKE3 hex), never heap-allocated.
- Errors are a Zig `error{...}` set mapped from the 8 stable C ABI codes
  (`IoError`, `HashMismatch`, `StoreError`, `InFlux`, `CatalogError`, `InvalidId`,
  `InvalidStore`, `Conflict`) plus `OutOfMemory`.

## Native library

This is a CGo-style binding: it links the native snapdir FFI static library.
The C ABI header (`snapdir.h`) and the static lib (`libsnapdir_ffi.a`) are produced
by `cargo build --release -p snapdir-ffi` and vendored into `bindings/zig/{include,lib}/`
at build time (they are gitignored, not checked in). Because that library is built
for one CPU arch, `build.zig` links it with the matching target (see the build.zig
NOTE). Per-OS/arch prebuilt libraries are deferred to credited release CI.

### Building the C ABI from the published crate (crates.io)

There is no separate Zig package registry blob for the native lib — the published
artifact is the [`snapdir-ffi`](https://crates.io/crates/snapdir-ffi) **source
crate**. Build it and generate the header (the crate ships `build = false` + no
header, so run `cbindgen`), then place them in `include/` + `lib/`:

```sh
# needs a Rust toolchain + cbindgen; on Alpine also: apk add libgcc
cargo build --release                       # -> libsnapdir_ffi.a
cbindgen --config cbindgen.toml --crate snapdir-ffi --output include/snapdir.h .
```

> **Alpine/musl note:** the Rust staticlib links `libgcc_s`; Alpine ships only
> the versioned `libgcc_s.so.1`, so `ln -sf /usr/lib/libgcc_s.so.1
> /usr/lib/libgcc_s.so` and build **native** (no `-Dtarget`, since the container
> is already the musl target). Verified by the `zig` job in
> `.github/workflows/verify-published.yml`.

## Consume as a Zig package

`build.zig.zon` declares the package; consumers reference it by content hash:

```sh
zig fetch --save git+https://…/snapdir#<rev>     # or a path, prints the hash
```

```zig
// consumer build.zig
const snapdir_dep = b.dependency("snapdir", .{ .target = target, .optimize = optimize });
exe.root_module.addImport("snapdir", snapdir_dep.module("snapdir"));
// + link the vendored libsnapdir_ffi.a (see build.zig NOTE).
```

## Usage

```zig
const std = @import("std");
const snapdir = @import("snapdir");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    const a = gpa.allocator();

    // Walk a directory → 64-char snapshot id ([64]u8 value, no free).
    const id = try snapdir.id(a, "./my-dir", .{});
    std.debug.print("{s}\n", .{id});

    // Full manifest (raw text + parsed entries); free with deinit.
    var m = try snapdir.manifest(a, "./my-dir", .{});
    defer m.deinit(a);
    for (m.entries) |e| {
        // e.type (PathType), e.perm (u32), e.checksum ([64]u8), e.size (u64), e.path
    }
    // snapdir.idFromManifest(m) == id

    // Options.
    const id2 = try snapdir.id(a, "./my-dir", .{ .no_follow = true, .exclude = "\\.tmp$" });
    _ = id2;

    // Transfer (blocking).
    const pushed = try snapdir.push(a, "./my-dir", "file:///tmp/store", .{});
    try snapdir.pull(a, &pushed, "file:///tmp/store", "./restored", .{});
}
```

## Error handling

```zig
const id = snapdir.id(a, "/no/such/path", .{}) catch |err| switch (err) {
    error.IoError => { /* … */ return; },
    else => return err,
};
```

## License

MIT — see [LICENSE](LICENSE).
