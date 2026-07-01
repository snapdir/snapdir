// Package snapdir provides CGo bindings over the snapdir-ffi C ABI.
//
// All blocking operations (fetch, push, pull, sync, verify, checkout) run the
// underlying C call on a goroutine so the caller's context.Context cancellation
// is honoured via a select on ctx.Done() and the result channel.
// The C operation runs to completion on its goroutine — acceptable for
// object-store lifetimes where partial cancellation would leave state
// inconsistent.
//
// Nil option pointers are accepted everywhere (defaults apply).
// No function panics; every failure is returned as *SnapdirError.
//
// Missing file:// stores are treated as empty (no error) — absent-store-is-empty
// is a design property of the underlying snapdir_diff_json C call.
package snapdir

/*
#cgo CFLAGS: -I${SRCDIR}/include
#cgo LDFLAGS: -L${SRCDIR}/lib -lsnapdir_ffi -lpthread -ldl -lm
#include "snapdir.h"
#include <stdlib.h>
*/
import "C"
import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"unsafe"
)

// initOnce ensures snapdir_init is called exactly once per process.
var initOnce sync.Once

// doInit calls snapdir_init if not yet called.
func doInit() {
	initOnce.Do(func() {
		C.snapdir_init()
	})
}

// PathType is a byte constant identifying the type of a manifest entry.
// Values: 'D' for directory, 'F' for file, 'L' for symlink.
type PathType byte

const (
	// PathTypeDir is a directory entry.
	PathTypeDir PathType = 'D'
	// PathTypeFile is a regular file entry.
	PathTypeFile PathType = 'F'
	// PathTypeSymlink is a symbolic link entry.
	PathTypeSymlink PathType = 'L'
)

// DiffStatus is a byte constant indicating a diff entry's change status.
// Values: 'A' added, 'D' deleted, 'M' modified, '=' unchanged.
type DiffStatus byte

const (
	// DiffStatusAdded indicates the path was added.
	DiffStatusAdded DiffStatus = 'A'
	// DiffStatusDeleted indicates the path was deleted.
	DiffStatusDeleted DiffStatus = 'D'
	// DiffStatusModified indicates the path was modified.
	DiffStatusModified DiffStatus = 'M'
	// DiffStatusUnchanged indicates the path is unchanged.
	DiffStatusUnchanged DiffStatus = '='
)

// ManifestOptions controls the walk behaviour for Manifest and ID.
type ManifestOptions struct {
	// NoFollow disables following of symbolic links during the walk.
	NoFollow bool
	// Absolute emits absolute paths instead of ./-relative paths.
	Absolute bool
	// Exclude is a list of extended-regex patterns; matching paths are omitted.
	// Multiple patterns are combined as a single alternation regex.
	Exclude []string
}

// ManifestEntry is a single line from the manifest (directory or file).
type ManifestEntry struct {
	// Path is the entry path as recorded in the manifest.
	Path string
	// PathType is the entry type ('D', 'F', or 'L').
	PathType PathType
	// Permissions is the POSIX mode bits in octal (e.g. 0644).
	Permissions uint32
	// Checksum is the 64-char BLAKE3 hex hash (empty for directories).
	Checksum string
	// Size is the byte count of the entry.
	Size uint64
}

// ManifestResult holds the parsed result of a directory walk.
// The Manifest function returns *ManifestResult.
type ManifestResult struct {
	// Entries is the list of parsed manifest lines.
	Entries []ManifestEntry
	// Raw is the full manifest text returned by the C library.
	Raw string
}

// DiffEntry is a single entry from a store diff operation.
type DiffEntry struct {
	// Status is the change status ('A', 'D', 'M', or '=').
	Status DiffStatus
	// Path is the entry path.
	Path string
}

// SnapdirError carries the stable error code and human-readable message from the C ABI.
type SnapdirError struct {
	// Code is one of the 8 stable C-ABI error codes or "INTERNAL".
	Code string
	// Message is the human-readable description of the error.
	Message string
}

// Error implements the error interface.
func (e *SnapdirError) Error() string {
	return fmt.Sprintf("[%s] %s", e.Code, e.Message)
}

// Version returns the snapdir-api version string (e.g. "1.10.0").
//
// The underlying C string has static lifetime and is never freed.
func Version() string {
	doInit()
	return C.GoString(C.snapdir_version())
}

// extractCError reads an error from a C SnapdirError out-parameter, frees it,
// and returns a *SnapdirError. Returns nil if errPtr is nil.
func extractCError(errPtr *C.SnapdirError) *SnapdirError {
	if errPtr == nil {
		return nil
	}
	code := C.GoString(C.snapdir_error_code(errPtr))
	msg := C.GoString(C.snapdir_error_message(errPtr))
	C.snapdir_error_free(errPtr)
	return &SnapdirError{Code: code, Message: msg}
}

// excludePattern joins a slice of patterns into a single alternation regex.
// Returns empty string if patterns is empty (passes NULL to the C layer).
func excludePattern(patterns []string) string {
	if len(patterns) == 0 {
		return ""
	}
	if len(patterns) == 1 {
		return patterns[0]
	}
	parts := make([]string, len(patterns))
	for i, p := range patterns {
		parts[i] = "(?:" + p + ")"
	}
	return strings.Join(parts, "|")
}

// parseManifestText parses manifest text into []ManifestEntry.
// Each non-comment, non-blank line has the format: TYPE PERM CHECKSUM SIZE PATH
func parseManifestText(text string) []ManifestEntry {
	var entries []ManifestEntry
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimRight(line, "\r")
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		parts := strings.SplitN(line, " ", 5)
		if len(parts) < 5 {
			continue
		}
		pt := PathType(parts[0][0])
		perm64, err := strconv.ParseUint(parts[1], 8, 32)
		if err != nil {
			continue
		}
		checksum := parts[2]
		size, err := strconv.ParseUint(parts[3], 10, 64)
		if err != nil {
			continue
		}
		path := parts[4]
		entries = append(entries, ManifestEntry{
			Path:        path,
			PathType:    pt,
			Permissions: uint32(perm64),
			Checksum:    checksum,
			Size:        size,
		})
	}
	return entries
}

// Manifest walks path and returns the parsed directory manifest.
// opts may be nil (defaults apply). Cancellation via ctx is honoured.
func Manifest(ctx context.Context, path string, opts *ManifestOptions) (*ManifestResult, error) {
	doInit()
	if err := ctx.Err(); err != nil {
		return nil, err
	}

	type manifestResult struct {
		m   *ManifestResult
		err error
	}
	ch := make(chan manifestResult, 1)

	go func() {
		cPath := C.CString(path)
		defer C.free(unsafe.Pointer(cPath))

		var excludeStr string
		var absolute, noFollow bool
		if opts != nil {
			excludeStr = excludePattern(opts.Exclude)
			absolute = opts.Absolute
			noFollow = opts.NoFollow
		}

		var cExclude *C.char
		if excludeStr != "" {
			cExclude = C.CString(excludeStr)
			defer C.free(unsafe.Pointer(cExclude))
		}

		var errPtr *C.SnapdirError
		raw := C.snapdir_manifest(
			cPath,
			cExclude,
			0,                 // walk_jobs: 0 = auto
			C.bool(absolute),
			C.bool(noFollow),
			nil,               // checksum_bin: NULL = b3sum default
			nil,               // cache_dir: NULL = default
			nil,               // catalog: NULL = default
			&errPtr,
		)
		if raw == nil {
			ch <- manifestResult{nil, extractCError(errPtr)}
			return
		}
		text := C.GoString(raw)
		C.snapdir_string_free(raw)
		entries := parseManifestText(text)
		ch <- manifestResult{&ManifestResult{Entries: entries, Raw: text}, nil}
	}()

	select {
	case res := <-ch:
		return res.m, res.err
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

// ID computes the snapshot id for the directory at path.
// opts may be nil (defaults apply). When opts sets Absolute or NoFollow (which
// snapdir_id does not accept), the id is derived from manifest text.
// Cancellation via ctx is honoured.
func ID(ctx context.Context, path string, opts *ManifestOptions) (string, error) {
	doInit()
	if err := ctx.Err(); err != nil {
		return "", err
	}

	needsManifestPath := opts != nil && (opts.Absolute || opts.NoFollow)

	type idResult struct {
		id  string
		err error
	}
	ch := make(chan idResult, 1)

	go func() {
		if needsManifestPath {
			// snapdir_id does not expose absolute/no_follow; compute via manifest.
			var excludeStr string
			if opts != nil {
				excludeStr = excludePattern(opts.Exclude)
			}
			cPath := C.CString(path)
			defer C.free(unsafe.Pointer(cPath))

			var cExclude *C.char
			if excludeStr != "" {
				cExclude = C.CString(excludeStr)
				defer C.free(unsafe.Pointer(cExclude))
			}

			var errPtr *C.SnapdirError
			rawManifest := C.snapdir_manifest(
				cPath, cExclude, 0,
				C.bool(opts.Absolute), C.bool(opts.NoFollow),
				nil, nil, nil, &errPtr,
			)
			if rawManifest == nil {
				ch <- idResult{"", extractCError(errPtr)}
				return
			}
			text := C.GoString(rawManifest)
			C.snapdir_string_free(rawManifest)

			cText := C.CString(text)
			defer C.free(unsafe.Pointer(cText))
			var errPtr2 *C.SnapdirError
			idRaw := C.snapdir_id_from_manifest_text(cText, &errPtr2)
			if idRaw == nil {
				ch <- idResult{"", extractCError(errPtr2)}
				return
			}
			id := C.GoString(idRaw)
			C.snapdir_string_free(idRaw)
			ch <- idResult{id, nil}
			return
		}

		// Standard: use snapdir_id directly.
		var excludeStr string
		if opts != nil {
			excludeStr = excludePattern(opts.Exclude)
		}
		cPath := C.CString(path)
		defer C.free(unsafe.Pointer(cPath))

		var cExclude *C.char
		if excludeStr != "" {
			cExclude = C.CString(excludeStr)
			defer C.free(unsafe.Pointer(cExclude))
		}

		var errPtr *C.SnapdirError
		idRaw := C.snapdir_id(cPath, cExclude, 0, nil, &errPtr)
		if idRaw == nil {
			ch <- idResult{"", extractCError(errPtr)}
			return
		}
		id := C.GoString(idRaw)
		C.snapdir_string_free(idRaw)
		ch <- idResult{id, nil}
	}()

	select {
	case res := <-ch:
		return res.id, res.err
	case <-ctx.Done():
		return "", ctx.Err()
	}
}

// IDFromManifest computes the snapshot id from a previously-computed ManifestResult.
// This is a pure synchronous function (no ctx).
func IDFromManifest(m *ManifestResult) (string, error) {
	doInit()
	cText := C.CString(m.Raw)
	defer C.free(unsafe.Pointer(cText))

	var errPtr *C.SnapdirError
	idRaw := C.snapdir_id_from_manifest_text(cText, &errPtr)
	if idRaw == nil {
		return "", extractCError(errPtr)
	}
	id := C.GoString(idRaw)
	C.snapdir_string_free(idRaw)
	return id, nil
}

// Push stages the directory at path and pushes it to storeURI.
// Returns the 64-char hex snapshot id on success.
// Cancellation via ctx is honoured.
func Push(ctx context.Context, path, storeURI string) (string, error) {
	doInit()
	if err := ctx.Err(); err != nil {
		return "", err
	}

	type pushResult struct {
		id  string
		err error
	}
	ch := make(chan pushResult, 1)

	go func() {
		cPath := C.CString(path)
		defer C.free(unsafe.Pointer(cPath))
		cStore := C.CString(storeURI)
		defer C.free(unsafe.Pointer(cStore))

		var errPtr *C.SnapdirError
		idRaw := C.snapdir_push_blocking(
			cPath,
			nil,    // source_id: NULL (using source_path)
			cStore,
			0,      // jobs: 0 = default
			nil,    // limit_rate: NULL = unlimited
			0,      // max_retries: 0 = default (5)
			nil,    // cache_dir: NULL = default
			&errPtr,
		)
		if idRaw == nil {
			ch <- pushResult{"", extractCError(errPtr)}
			return
		}
		id := C.GoString(idRaw)
		C.snapdir_string_free(idRaw)
		ch <- pushResult{id, nil}
	}()

	select {
	case res := <-ch:
		return res.id, res.err
	case <-ctx.Done():
		return "", ctx.Err()
	}
}

// Pull fetches a snapshot from storeURI and materializes it into dest.
// Cancellation via ctx is honoured.
func Pull(ctx context.Context, snapshotID, storeURI, dest string) error {
	doInit()
	if err := ctx.Err(); err != nil {
		return err
	}

	type pullResult struct {
		err error
	}
	ch := make(chan pullResult, 1)

	go func() {
		cID := C.CString(snapshotID)
		defer C.free(unsafe.Pointer(cID))
		cStore := C.CString(storeURI)
		defer C.free(unsafe.Pointer(cStore))
		cDest := C.CString(dest)
		defer C.free(unsafe.Pointer(cDest))

		var errPtr *C.SnapdirError
		rc := C.snapdir_pull_blocking(
			cID,
			cStore,
			cDest,
			false, // delete_extra: false = default
			0,     // jobs: 0 = default
			&errPtr,
		)
		if rc != 0 {
			ch <- pullResult{extractCError(errPtr)}
			return
		}
		ch <- pullResult{nil}
	}()

	select {
	case res := <-ch:
		return res.err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// Fetch fetches a snapshot from storeURI into the local cache.
// Cancellation via ctx is honoured.
func Fetch(ctx context.Context, snapshotID, storeURI string) error {
	doInit()
	if err := ctx.Err(); err != nil {
		return err
	}

	type fetchResult struct {
		err error
	}
	ch := make(chan fetchResult, 1)

	go func() {
		cID := C.CString(snapshotID)
		defer C.free(unsafe.Pointer(cID))
		cStore := C.CString(storeURI)
		defer C.free(unsafe.Pointer(cStore))

		var errPtr *C.SnapdirError
		rc := C.snapdir_fetch_blocking(cID, cStore, 0, &errPtr)
		if rc != 0 {
			ch <- fetchResult{extractCError(errPtr)}
			return
		}
		ch <- fetchResult{nil}
	}()

	select {
	case res := <-ch:
		return res.err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// diffEntryJSON is the raw JSON shape returned by snapdir_diff_json.
type diffEntryJSON struct {
	Status string `json:"status"`
	Path   string `json:"path"`
}

// Diff diffs two stores and returns the change entries.
// fromURI and toURI are store URIs (e.g. "file:///tmp/store").
// An absent file:// store is treated as empty (no error). An invalid or
// unknown-scheme URI returns INVALID_STORE from the C layer.
// Cancellation via ctx is honoured.
func Diff(ctx context.Context, fromURI, toURI string) ([]DiffEntry, error) {
	doInit()
	if err := ctx.Err(); err != nil {
		return nil, err
	}

	type diffResult struct {
		entries []DiffEntry
		err     error
	}
	ch := make(chan diffResult, 1)

	go func() {
		cFrom := C.CString(fromURI)
		defer C.free(unsafe.Pointer(cFrom))
		cTo := C.CString(toURI)
		defer C.free(unsafe.Pointer(cTo))

		// Build NULL-terminated C arrays for from_uris and to_uris.
		fromArr := [2]*C.char{cFrom, nil}
		toArr := [2]*C.char{cTo, nil}

		var errPtr *C.SnapdirError
		jsonRaw := C.snapdir_diff_json(
			(**C.char)(unsafe.Pointer(&fromArr[0])),
			(**C.char)(unsafe.Pointer(&toArr[0])),
			nil,   // id: NULL
			false, // include_unchanged: false
			nil,   // on_conflict: NULL = "error"
			&errPtr,
		)
		if jsonRaw == nil {
			ch <- diffResult{nil, extractCError(errPtr)}
			return
		}
		jsonStr := C.GoString(jsonRaw)
		C.snapdir_string_free(jsonRaw)

		var raw []diffEntryJSON
		if err := json.Unmarshal([]byte(jsonStr), &raw); err != nil {
			ch <- diffResult{nil, &SnapdirError{Code: "INTERNAL", Message: "diff JSON unmarshal: " + err.Error()}}
			return
		}
		entries := make([]DiffEntry, len(raw))
		for i, r := range raw {
			status := DiffStatus('=')
			if len(r.Status) > 0 {
				status = DiffStatus(r.Status[0])
			}
			entries[i] = DiffEntry{Status: status, Path: r.Path}
		}
		ch <- diffResult{entries, nil}
	}()

	select {
	case res := <-ch:
		return res.entries, res.err
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}
