# github.com/snapdir/snapdir/bindings/go

Go CGo bindings for [snapdir](https://snapdir.org) — a content-addressed directory snapshot tool
that walks a directory tree, hashes every file with BLAKE3, and produces a stable snapshot ID you
can push to and pull from object stores (file, S3, GCS, Backblaze B2). Results are bit-identical
to the snapdir CLI and Rust core via the C ABI (`libsnapdir_ffi`).

## Install / Build

> **This is a CGo package.** It links the native `libsnapdir_ffi` static library and requires
> the native artefacts to be present at build time. A plain `go get` without the library in place
> will fail with linker errors.

### Build the native library

The static library (`lib/libsnapdir_ffi.a`) and header (`include/snapdir.h`) are produced by the
snapdir Rust workspace and vendored into `bindings/go/{lib,include}/` at build time. They are
gitignored and are **not** checked in to the repository.

```sh
# From the repo root — requires Rust toolchain (cargo)
cargo build --release -p snapdir-ffi --locked

# Copy artefacts into the Go package
mkdir -p bindings/go/lib bindings/go/include
cp include/snapdir.h bindings/go/include/
cp target/release/libsnapdir_ffi.a bindings/go/lib/

# Now the Go package can be built
cd bindings/go
go build ./...
```

### CGo environment

The package uses the following CGo flags (set automatically via `#cgo` directives in `snapdir.go`):

```
CFLAGS:  -I${SRCDIR}/include
LDFLAGS: -L${SRCDIR}/lib -lsnapdir_ffi -lpthread -ldl -lm
```

When building inside the project's Docker image (amd64-Go on arm64 host):

```sh
export CGO_ENABLED=1 GOARCH=arm64 CC=gcc GOFLAGS=-buildvcs=false
```

> **Note:** Per-GOOS/GOARCH prebuilt libraries and a standalone `github.com/snapdir/go-snapdir`
> mirror (usable without a local Rust build) are deferred to a future release CI pipeline.

## Usage

```go
import (
    "context"
    "errors"
    "fmt"
    "log"

    snapdir "github.com/snapdir/snapdir/bindings/go"
)

func main() {
    ctx := context.Background()

    // Compute a snapshot ID for a directory (64-char lowercase hex BLAKE3)
    id, err := snapdir.ID(ctx, "./my-dir", nil)
    if err != nil {
        log.Fatal(err)
    }
    fmt.Println("snapshot:", id)

    // Full manifest with per-entry type, permissions, checksums, and sizes
    result, err := snapdir.Manifest(ctx, "./my-dir", nil)
    if err != nil {
        log.Fatal(err)
    }
    for _, entry := range result.Entries {
        fmt.Printf("%c %04o %s %d %s\n",
            entry.PathType, entry.Permissions, entry.Checksum, entry.Size, entry.Path)
    }

    // ManifestOptions: control walk behaviour
    opts := &snapdir.ManifestOptions{
        NoFollow: true,                        // record symlinks as links, not targets
        Absolute: false,                       // use ./-relative paths (default)
        Exclude:  []string{`\.git$`, `\.DS_Store`}, // extended-regex exclude patterns
    }
    result, err = snapdir.Manifest(ctx, "./my-dir", opts)
    if err != nil {
        log.Fatal(err)
    }
    _ = result

    // Push a directory to a store; returns the snapshot ID
    sid, err := snapdir.Push(ctx, "./my-dir", "file:///tmp/my-store")
    if err != nil {
        log.Fatal(err)
    }
    fmt.Println("pushed:", sid)

    // Pull a snapshot from a store and materialize it at dest
    err = snapdir.Pull(ctx, sid, "file:///tmp/my-store", "./restored")
    if err != nil {
        log.Fatal(err)
    }

    // Diff two stores (absent file:// store is treated as empty)
    entries, err := snapdir.Diff(ctx, "file:///tmp/store-a", "file:///tmp/store-b")
    if err != nil {
        log.Fatal(err)
    }
    for _, e := range entries {
        fmt.Printf("%c %s\n", e.Status, e.Path) // A/D/M/=
    }

    // Context cancellation is honoured by all blocking operations
    ctx2, cancel := context.WithTimeout(context.Background(), 30*1e9)
    defer cancel()
    _, err = snapdir.ID(ctx2, "./large-dir", nil)
    if errors.Is(err, context.DeadlineExceeded) {
        fmt.Println("timed out")
    }

    // Typed error handling — SnapdirError.Code is one of 8 stable SCREAMING_SNAKE_CASE codes
    _, err = snapdir.Push(ctx, "./missing", "file:///tmp/my-store")
    var se *snapdir.SnapdirError
    if errors.As(err, &se) {
        fmt.Println("error code:", se.Code)    // e.g. "IO_ERROR"
        fmt.Println("error message:", se.Message)
    }
}
```

## API

| Function | Signature | Notes |
|---|---|---|
| `ID` | `ID(ctx, path string, opts *ManifestOptions) (string, error)` | 64-hex BLAKE3 snapshot ID |
| `Manifest` | `Manifest(ctx, path string, opts *ManifestOptions) (*ManifestResult, error)` | Full entry list + raw text |
| `IDFromManifest` | `IDFromManifest(m *ManifestResult) (string, error)` | Sync, pure — same result as `ID()` |
| `Push` | `Push(ctx, path, storeURI string) (string, error)` | Upload to store; returns snapshot ID |
| `Pull` | `Pull(ctx, snapshotID, storeURI, dest string) error` | Download + materialize |
| `Fetch` | `Fetch(ctx, snapshotID, storeURI string) error` | Download to local cache |
| `Diff` | `Diff(ctx, fromURI, toURI string) ([]DiffEntry, error)` | Compare two stores |
| `Version` | `Version() string` | snapdir-api crate version string |

**`ManifestOptions`**: `NoFollow bool`, `Absolute bool`, `Exclude []string`.

**`ManifestEntry`**: `Path string`, `PathType PathType` (`'D'`/`'F'`/`'L'`), `Permissions uint32`,
`Checksum string` (64-char BLAKE3 hex, empty for directories), `Size uint64`.

**`DiffEntry`**: `Status DiffStatus` (`'A'` added, `'D'` deleted, `'M'` modified, `'='` unchanged),
`Path string`.

**`SnapdirError`**: implements `error`; stable `.Code` string (one of: `IO_ERROR`, `HASH_MISMATCH`,
`STORE_ERROR`, `IN_FLUX`, `CATALOG_ERROR`, `INVALID_ID`, `INVALID_STORE`, `CONFLICT`).

## License

MIT
