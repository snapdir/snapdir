# snapdir-api

The stable, async facade for [snapdir](https://github.com/snapdir/snapdir) —
content-addressable directory snapshots — that every language binding consumes.

This crate wraps `snapdir-core`, `snapdir-stores`, and `snapdir-catalog` behind a
single, documented, semver-stable surface:

- Async distribution functions (`push`, `fetch`, `pull`, `checkout`, `sync`, `diff`,
  `verify`) that `spawn_blocking` over the synchronous stores runtime, plus the sync
  snapshot operations (`manifest`, `id`, `id_from_manifest`, `stage`).
- Typed newtypes and options structs (`SnapshotId`, `StoreUri`, `Manifest`,
  `ManifestEntry`, `DiffEntry`, `ManifestOptions`, transfer/diff options).
- A typed `SnapdirError` enum — all public functions return `Result<T, SnapdirError>`;
  no `anyhow` leaks into the public surface.

`snapdir-core` stays pure (no tokio); the async layer lives here. The Node and Python
bindings depend on this crate directly; the C ABI (`snapdir-ffi`) and the Go/C·C++/Zig/
Java bindings wrap it through that ABI.

It is part of the snapdir project. Full documentation, the CLI, and the available
storage backends are at **[snapdir.org](https://snapdir.org)**; the source lives in the
[canonical repository](https://github.com/snapdir/snapdir).

## License

MIT
