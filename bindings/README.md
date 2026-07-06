# snapdir language bindings

Use snapdir from your language of choice. Every binding wraps the same Rust core
(`snapdir-api` → `snapdir-core`), so **manifests and snapshot IDs are
bit-identical** across all of them and the `snapdir` CLI — content hashed with
BLAKE3, pushed to and pulled from the same object stores (file, S3, GCS,
Backblaze B2, SSH).

| Language | Package | Install | Registry |
|---|---|---|---|
| **Node.js** | [`@snapdir/snapdir`](node/) | `npm install @snapdir/snapdir` | [npm](https://www.npmjs.com/package/@snapdir/snapdir) |
| **Python** | [`snapdir`](python/) | `pip install snapdir` | [PyPI](https://pypi.org/project/snapdir/) |
| **Go** | [`bindings/go`](go/) | `go get github.com/snapdir/snapdir/bindings/go` | GOPROXY |
| **Java** | [`org.snapdir:snapdir`](java/) | Maven / Gradle coordinate | [Maven Central](https://central.sonatype.com/artifact/org.snapdir/snapdir) |
| **C / C++** | [`snapdir.hpp` + `snapdir-ffi`](cpp/) | crates.io C ABI + RAII header | [crates.io](https://crates.io/crates/snapdir-ffi) |
| **Zig** | [`bindings/zig`](zig/) | crates.io C ABI + Zig wrapper | [crates.io](https://crates.io/crates/snapdir-ffi) |
| **Rust** | [`snapdir-api`](../crates/snapdir-api) · [`snapdir-ffi`](../crates/snapdir-ffi) | `cargo add snapdir-api` | [crates.io](https://crates.io/crates/snapdir-api) |

Each binding exposes the same core surface — `id` / `manifest`, and async
`push` / `pull` / `fetch` / `diff` / `sync` — in the idiom of its language
(`bigint`/`int`/`long` sizes, typed error hierarchies, `Promise`/`async`/
`CompletableFuture`/`context.Context`/`std::future`). See each subdirectory's
README for the language-specific API and examples.

## Two install shapes

- **Prebuilt, no compiler** — **Node** (`npm`) and **Python** (`pip`) ship native
  binaries (linux gnu + **musl**, macOS; x64 + arm64) and **Java** ships a jar
  with the native library embedded. Install and go.
- **Build from source** — **C/C++**, **Zig**, and **Go** consume the C ABI by
  building the [`snapdir-ffi`](https://crates.io/crates/snapdir-ffi) crate from
  crates.io (needs a Rust toolchain + `cbindgen`); the Go module additionally
  resolves from GOPROXY. See each README for the exact recipe.

## Platform support

All bindings run on Linux (glibc **and Alpine/musl**, x64 + arm64) and macOS.
**Exception:** the Java jar is currently **glibc-only** — it does not load on
Alpine/musl yet (planned for 1.11.1). Windows is not supported (snapdir is
Unix-only).

Every binding is continuously verified **from its live public registry** on
blank-slate base images by
[`.github/workflows/verify-published.yml`](../.github/workflows/verify-published.yml):
each installs the published package, runs the canonical example, and asserts the
snapshot ID is byte-exact to the reference CLI.

## Examples

Runnable, idiomatic example apps for every language live in
[`examples/`](../examples) — a tiny `push` / `pull` / `id` / `diff` CLI over each
binding's API, the same programs the verification workflow runs.
