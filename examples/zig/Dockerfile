# Blank-slate Zig image — installs ONLY the packaged snapdir Zig binding.
# Build stage: debian:bookworm-slim + zig 0.13 (x86_64) cross-compiles to
# aarch64-linux-gnu (matches the arm64 libsnapdir_ffi.a ABI).
# Final stage: minimal debian:bookworm-slim with just the aarch64 binary.
# On Apple Silicon (arm64 kernel) the aarch64 binary runs natively.
# On CI (amd64 Linux with qemu binfmt_misc) the aarch64 binary runs via qemu.

# ── Build stage ────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl xz-utils ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install the x86_64 zig 0.13 compiler (runs on the amd64 build host and
# cross-compiles to aarch64-linux-gnu matching libsnapdir_ffi.a).
RUN curl -fsSL https://ziglang.org/download/0.13.0/zig-linux-x86_64-0.13.0.tar.xz \
        | tar -xJ -C /usr/local \
    && ln -s /usr/local/zig-linux-x86_64-0.13.0/zig /usr/local/bin/zig

WORKDIR /build

# Copy the vendored snapdir Zig binding (staged by run_relay.sh).
COPY src/ src/
COPY include/ include/
COPY lib/ lib/

# Copy the example app and its build script.
COPY app.zig ./
COPY build.zig ./

# Cross-compile to aarch64-linux-gnu (matches the arm64 libsnapdir_ffi.a ABI).
RUN zig build -Doptimize=ReleaseSafe -Dtarget=aarch64-linux-gnu

# ── Final stage ────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

# Copy only the compiled aarch64 binary — no Zig toolchain, no Rust, no sources.
COPY --from=builder /build/zig-out/bin/app /app

ENTRYPOINT ["/app"]
