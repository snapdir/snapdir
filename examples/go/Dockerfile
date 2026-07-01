# Blank-slate Go image — installs the snapdir Go binding via vendored source + CGo.
# golang:1.24-bookworm has gcc (required for CGo); no additional toolchain needed.
# The binding is a source module (no registry); the static lib (libsnapdir_ffi.a)
# and the C ABI header are pre-built and vendored by run_relay.sh.

FROM golang:1.24-bookworm

WORKDIR /build

# Copy the vendored snapdir Go module (source + pre-built lib + header).
# Staged by run_relay.sh from the proven go-pack recipe output.
COPY snapdir-go/ snapdir-go/

# Copy the canonical example app.
COPY app.go ./

# Create a module with a local replace directive pointing at the vendored binding.
RUN printf 'module snapdir-app\n\ngo 1.22\n\nrequire github.com/snapdir/snapdir/bindings/go v0.0.0\n\nreplace github.com/snapdir/snapdir/bindings/go => ./snapdir-go\n' > go.mod

ENV CGO_ENABLED=1
RUN go build -o /app .

ENTRYPOINT ["/app"]
