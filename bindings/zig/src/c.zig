// c.zig — C interop layer for snapdir-ffi.
//
// This file is INTERNAL to the binding.  It is imported only by src/snapdir.zig.
// Callers of the public surface MUST NOT import this file directly; the C ABI
// symbols are not part of the idiomatic Zig API.
//
// The translated-C namespace is exported as a single `pub const c` so that
// snapdir.zig can reference it as `c.snapdir_version()`, etc., while keeping
// the raw C names fully hidden from downstream callers.
//
// Include path: bindings/zig/include/snapdir.h
// (vendored by the verification step before `zig build`).
pub const c = @cImport({
    @cInclude("snapdir.h");
});
