# snapdir (C++)

Header-only **C++ RAII bindings** for [snapdir](https://snapdir.org) — a
content-addressed directory snapshot tool that walks a directory tree, hashes
every file with BLAKE3, and produces a stable snapshot ID you can push to and
pull from object stores (file, S3, GCS, Backblaze B2, SSH).

The bindings are a thin RAII wrapper (`snapdir.hpp`) over the snapdir **C ABI**
(`snapdir.h`), which itself wraps the canonical `snapdir-api` core — so manifests
and snapshot IDs are **bit-identical** to the `snapdir` CLI and to the Node,
Python, and Go bindings.

## Requirements

- A C++17 compiler (C++20 tested). Uses `std::optional`, `std::filesystem`,
  `std::future`.
- The native snapdir FFI library. The C ABI header (`snapdir.h`) and the static
  library (`libsnapdir_ffi.a`) are produced by:

  ```sh
  cargo build --release -p snapdir-ffi
  ```

  In this repo they are vendored into `bindings/cpp/{include,lib}/` at build time
  (they are gitignored, not checked in). Per-OS/arch prebuilt libraries and a
  CMake `FindSnapdir`/`FetchContent` integration are deferred to release CI; this
  package builds with the native toolchain via the included `Makefile`.

## Build & install

```sh
# vendor the C header + static lib, then install the header(s), lib and a
# pkg-config file under a prefix:
make -C bindings/cpp install PREFIX=/usr/local
pkg-config --cflags --libs snapdir      # -I/usr/local/include -L/usr/local/lib -lsnapdir_ffi
```

Compile a program against it:

```sh
clang++ -std=c++20 main.cpp $(pkg-config --cflags --libs snapdir) -o main
# or: g++ -std=c++20 main.cpp $(pkg-config --cflags --libs snapdir) -o main
```

## Usage

```cpp
#include <snapdir.hpp>
#include <cstdio>

int main() {
    // Walk a directory and get its 64-char snapshot id.
    const std::string id = snapdir::id("./my-dir");
    std::printf("%s\n", id.c_str());

    // The full manifest (raw text + parsed entries).
    snapdir::Manifest m = snapdir::manifest("./my-dir");
    for (const snapdir::ManifestEntry &e : m.entries) {
        // e.type (PathType), e.perm (uint32), e.checksum, e.size (uint64), e.path
    }
    // id_from_manifest(m) == id("./my-dir")

    // Options (defaults match the C ABI):
    snapdir::ManifestOptions opts;
    opts.no_follow = true;                 // do not follow symlinks
    opts.exclude = std::string("\\.tmp$"); // extended-regex exclusion
    const std::string id2 = snapdir::id("./my-dir", opts);

    // Async transfer ops return std::future<T>; .get() resolves or throws.
    const std::string pushed = snapdir::push("./my-dir", "file:///tmp/store").get();
    snapdir::pull(pushed, "file:///tmp/store", "./restored").get();
    snapdir::fetch(pushed, "file:///tmp/store").get();

    // Diff two stores.
    std::vector<snapdir::DiffEntry> changes =
        snapdir::diff("file:///tmp/store-a", "file:///tmp/store-b").get();

    return 0;
}
```

## Error handling

Every operation throws `snapdir::Error` (a `std::runtime_error`) on failure. The
RAII guards (`StringGuard`/`ErrorGuard`) release every C allocation even when an
exception is thrown.

```cpp
try {
    auto id = snapdir::id("/no/such/path");
} catch (const snapdir::Error &e) {
    // e.code() is one of the stable codes: IO_ERROR, HASH_MISMATCH, STORE_ERROR,
    // IN_FLUX, CATALOG_ERROR, INVALID_ID, INVALID_STORE, CONFLICT (or INTERNAL).
    std::fprintf(stderr, "snapdir error [%s]: %s\n", e.code().c_str(), e.what());
}
```

## License

MIT — see [LICENSE](LICENSE).
