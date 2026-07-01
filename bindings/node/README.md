# @snapdir/snapdir

Node.js bindings for [snapdir](https://snapdir.org) — a content-addressed directory snapshot tool that walks a directory tree, hashes every file with BLAKE3, and produces a stable snapshot ID you can push to and pull from object stores (file, S3, GCS, Backblaze B2).

## Install

```sh
npm install @snapdir/snapdir
```

## Usage

### ESM

```js
import { manifest, id, push, pull, stage, diff, SnapdirError } from '@snapdir/snapdir';

// Walk a directory and get its snapshot ID (async, never blocks the event loop)
const snapshotId = await id('./my-dir');
console.log(snapshotId); // 64-char lowercase hex

// Full manifest with per-entry checksums and sizes (size is always bigint)
const m = await manifest('./my-dir');
for (const entry of m.entries) {
  console.log(entry.path, entry.size); // size: bigint
}

// Push a directory snapshot to a store and get its ID back
const sid = await push('./my-dir', 'file:///tmp/my-store');

// Pull a snapshot from a store and materialize it at dest
await pull(snapshotId, 'file:///tmp/my-store', './restored');

// Error handling — SnapdirError.code is one of 8 stable SCREAMING_SNAKE_CASE codes
try {
  await push('./missing', 'file:///tmp/my-store');
} catch (err) {
  if (err instanceof SnapdirError) {
    console.error(err.code);    // e.g. "IO_ERROR"
    console.error(err.message);
  }
}
```

### CJS

```js
const { manifest, id, push, SnapdirError } = require('@snapdir/snapdir');

// Options: noFollow (record symlinks as links), absolute paths, exclude patterns
manifest('./my-dir', { noFollow: true, absolute: false, exclude: ['\\.git$', '\\.DS_Store'] })
  .then(m => console.log(m.raw));
```

## API

| Function | Returns | Notes |
|---|---|---|
| `manifest(path, opts?)` | `Promise<Manifest>` | Full entry list + raw text |
| `id(path, opts?)` | `Promise<SnapshotId>` | 64-hex snapshot ID |
| `idFromManifest(m)` | `SnapshotId` | Sync, pure — same result as `id()` |
| `stage(path)` | `Promise<SnapshotId>` | Walk + local cache write |
| `push(path, storeUri)` | `Promise<SnapshotId>` | Upload to store |
| `pull(snapshotId, storeUri, dest)` | `Promise<void>` | Download + materialize |
| `fetch(snapshotId, storeUri)` | `Promise<void>` | Download to local cache |
| `checkout(snapshotId, dest)` | `Promise<void>` | Materialize from local cache |
| `sync(snapshotId, srcUri, dstUri)` | `Promise<void>` | Copy between stores |
| `diff(params)` | `Promise<DiffEntry[]>` | `{from, to}` store URI arrays |
| `verify(snapshotId, storeUri)` | `Promise<VerifyResult>` | `{ok: boolean}` |
| `version()` | `string` | snapdir-api crate version |

`ManifestOptions`: `noFollow?: boolean`, `absolute?: boolean`, `exclude?: string[]`.

`size` on `ManifestEntry` is always `bigint` (u64 — can exceed `Number.MAX_SAFE_INTEGER`).

`SnapdirError` extends `Error` with a stable `.code` string (one of: `IO_ERROR`, `HASH_MISMATCH`, `STORE_ERROR`, `IN_FLUX`, `CATALOG_ERROR`, `INVALID_ID`, `INVALID_STORE`, `CONFLICT`).

## License

MIT
