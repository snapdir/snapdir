# snapdir example apps

This directory contains six small, idiomatic CLI apps — one per supported language
binding — that together form the canonical usage documentation for each binding.
Each app is built from its stack's **barebones base image** and installs **only the
packaged artifact** (npm tarball, Python wheel, jar, vendored Go module + C lib,
C++ header + lib, Zig package + lib).  No `snapdir-bindings:dev`, no in-tree rebuild.

They also double as the cross-language integration relay: a snapshot written by one
language's packaged binding is read, verified, and diffed by the others through a
shared S3 store (minio).

---

## Consumer matrix

The key packaging distinction is whether the consumer needs a language toolchain at
all, or just the runtime:

| Language | Base image | Binding package | Consumer needs |
|---|---|---|---|
| **Node** | `node:22-slim` | `@snapdir/snapdir` npm tarball | `npm install` only — tarball embeds a prebuilt `.node` native addon (zero build tools) |
| **Python** | `python:3.12-slim` | `snapdir` wheel | `pip install` only — wheel embeds a prebuilt `.abi3.so` (zero build tools) |
| **Java** | `eclipse-temurin:17-jdk` | `snapdir.jar` | JDK 17; jar embeds the native `.so` (NativeLoader extracts it at runtime); run with `--add-modules jdk.incubator.foreign --enable-native-access=ALL-UNNAMED` |
| **Go** | `golang:1.24-bookworm` | `github.com/snapdir/snapdir/bindings/go` | Go + gcc (CGo); vendored module source + `libsnapdir_ffi.a` + `snapdir.h` |
| **C++** | `gcc:13` | `snapdir.hpp` header-only wrapper | g++ -std=c++20; `snapdir.hpp`, `snapdir.h`, `libsnapdir_ffi.a` |
| **Zig** | `debian:bookworm-slim` + zig 0.13 | `snapdir` Zig package | zig 0.13; vendored binding source + `libsnapdir_ffi.a` + `snapdir.h` |

**Prebuilt-native (zero build tools):** Node, Python, Java — install the package and
run; no compiler or Rust toolchain needed.

**Source / compile-time (stack toolchain + vendored C lib):** Go, C++, Zig — the
consumer must have the language toolchain and link against the pre-built static
library (`libsnapdir_ffi.a`) plus the provided C ABI header.

---

## Per-language examples

### Node — [`examples/node/`](node/)

```
examples/node/
  app.mjs       — canonical usage example
  Dockerfile    — FROM node:22-slim; npm install ./snapdir.tgz
```

The `@snapdir/snapdir` npm tarball embeds a prebuilt `.node` native addon.
A real consumer only needs `npm install ./snapdir-*.tgz` on a bare `node:22-slim`
image — no Rust toolchain, no compiler.

### Python — [`examples/python/`](python/)

```
examples/python/
  app.py        — canonical usage example
  Dockerfile    — FROM python:3.12-slim; pip install snapdir-*.whl
```

The `snapdir` wheel embeds a prebuilt `.abi3.so` extension.
A real consumer only needs `pip install snapdir-*.whl` on a bare `python:3.12-slim`
image — no Rust toolchain, no compiler.

### Go — [`examples/go/`](go/)

```
examples/go/
  app.go        — canonical usage example
  Dockerfile    — FROM golang:1.24-bookworm; CGo build with vendored module
```

Uses the `github.com/snapdir/snapdir/bindings/go` module via CGo.  The binding
ships as Go source + a pre-built `libsnapdir_ffi.a` + `snapdir.h`; the consumer
links them with a standard `go build` (gcc must be available for CGo).

### C++ — [`examples/cpp/`](cpp/)

```
examples/cpp/
  app.cpp       — canonical usage example
  Dockerfile    — FROM gcc:13; g++ -std=c++20 app.cpp -I. -L. -lsnapdir_ffi ...
```

Uses the header-only RAII wrapper (`snapdir.hpp` + `snapdir.h`) over the C ABI.
The consumer needs g++ (or clang++) -std=c++20, the two headers, and
`libsnapdir_ffi.a`.

### Zig — [`examples/zig/`](zig/)

```
examples/zig/
  app.zig       — canonical usage example
  build.zig     — zig build script
  Dockerfile    — FROM debian:bookworm-slim + zig 0.13; cross-compile to aarch64
```

Uses the `snapdir` Zig package (vendored source + `libsnapdir_ffi.a` + `snapdir.h`).
The Dockerfile cross-compiles to `aarch64-linux-gnu` with the x86_64 zig 0.13
compiler (matches the arm64 `libsnapdir_ffi.a` ABI; the aarch64 binary runs on
Apple Silicon natively and on amd64 CI via qemu binfmt_misc).

### Java — [`examples/java/`](java/)

```
examples/java/
  App.java      — canonical usage example
  Dockerfile    — FROM eclipse-temurin:17-jdk; javac + java with incubator.foreign
```

The `snapdir.jar` embeds the native `.so` which NativeLoader extracts at runtime.
The consumer needs JDK 17 and must pass
`--add-modules jdk.incubator.foreign --enable-native-access=ALL-UNNAMED` to both
`javac` and `java`.  Uses the JDK 17 Foreign Function incubator API
(`CLinker`, `C_POINTER`, etc.) — not JNA.

---

## App CLI

Every example exposes the same four-command interface over the binding API
(not the `snapdir` CLI):

```
app push <dir> <store>              # stage dir, upload to store; prints 64-hex snapshot id
app pull <id> <store> <dest>        # fetch snapshot from store, materialise into dest
app id   <dir>                      # compute and print the 64-hex snapshot id
app diff <store@id_a> <store@id_b>  # print STATUS<TAB>PATH per diffed entry
```

The **store URI** (`s3://bucket/prefix`, `file:///path`, etc.) is always a CLI
argument — not an environment variable.  S3 endpoint and credentials come from
the environment:

```
SNAPDIR_S3_STORE_ENDPOINT_URL   # e.g. http://snapdir-minio:9000
AWS_ACCESS_KEY_ID
AWS_SECRET_ACCESS_KEY
AWS_DEFAULT_REGION              # defaults to us-east-1
```

---

## Running the cross-language relay

The relay is host-orchestrated: it spins up a Docker bridge network, a minio S3
service container, builds all six barebones app images, and runs the choreography
(Node pushes, Python round-trips, Go pulls, Java diffs, C++ and Zig verify):

```bash
bash tests/integration/run_relay.sh
```

See [`tests/integration/run_relay.sh`](../tests/integration/run_relay.sh) for the
full orchestrator.  Pass `--smoke` to verify only the network and S3 reach
end-to-end without building the app images.

The shared store URI used by the relay is `s3://snapdir-integ/relay-<pid>` served
by the `snapdir-minio` service container on the `snapdir-relay` Docker network.
Every assertion is byte-exact against the oracle values computed by the snapdir
Rust API over the same fixture bytes.
