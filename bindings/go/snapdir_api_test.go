// Black-box API spec for the snapdir Go binding (Phase 39, go-api-spec-tests).
//
// Authored from the SPEC ONLY (snapdir-api / C-ABI design + go.md idioms) with
// NO visibility into the Go binding's implementation. It is an EXTERNAL test
// (`package snapdir_test`) so it survives without touching production src/. It
// pins the idiomatic CGo-over-C-ABI Go surface and is expected to FAIL/not
// compile against the current scaffold (which exposes only Version()).
//
// Pinned contract surface (the impl MUST satisfy these signatures):
//
//	func Version() string
//	func Manifest(ctx context.Context, path string, opts *ManifestOptions) (*Manifest, error)
//	func ID(ctx context.Context, path string, opts *ManifestOptions) (string, error)
//	func IDFromManifest(m *Manifest) (string, error)   // pure/sync, no ctx
//	func Push(ctx context.Context, path, storeURI string) (string, error)
//	func Pull(ctx context.Context, snapshotID, storeURI, dest string) error
//	func Diff(ctx context.Context, fromURI, toURI string) ([]DiffEntry, error)
//
//	type ManifestOptions struct { NoFollow bool; Absolute bool; Exclude []string }
//	type Manifest struct { Entries []ManifestEntry; Raw string }
//	type ManifestEntry struct {
//		Path string; PathType PathType; Permissions uint32; Checksum string; Size uint64
//	}
//	type DiffEntry struct { Status DiffStatus; Path string }
//	type SnapdirError struct { Code string; Message string }  // implements error
//
//	type PathType byte   // 'D' dir, 'F' file, 'L' symlink
//	type DiffStatus byte // 'A' added, 'D' deleted, 'M' modified, '=' unchanged
package snapdir_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"testing"
	"time"

	snapdir "github.com/snapdir/snapdir/bindings/go"
)

// The 8 stable error codes from the C ABI (snapdir_error_code). A binding
// failure MUST surface its Code from exactly this set (or "INTERNAL" for a
// catch_unwind boundary, which we still treat as non-panicking).
var stableCodes = map[string]bool{
	"IO_ERROR":      true,
	"HASH_MISMATCH": true,
	"STORE_ERROR":   true,
	"IN_FLUX":       true,
	"CATALOG_ERROR": true,
	"INVALID_ID":    true,
	"INVALID_STORE": true,
	"CONFLICT":      true,
}

var hex64 = regexp.MustCompile(`^[0-9a-f]{64}$`)

// noPanic runs fn and fails the test if fn panics — pins the "NO panic in the
// binding layer; every failure is a returned error" contract (go.md).
func noPanic(t *testing.T, name string, fn func()) {
	t.Helper()
	defer func() {
		if r := recover(); r != nil {
			t.Fatalf("%s: binding layer panicked, must return an error instead: %v", name, r)
		}
	}()
	fn()
}

// buildTree writes a small offline temp tree (no network needed) and returns
// its root. Local Manifest/ID operate purely on the filesystem.
func buildTree(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "sub", "b.bin"), []byte("world!\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return root
}

// buildSlowTree writes thousands of files so a Manifest walk is slow enough
// that a 1ms ctx deadline reliably fires before completion.
func buildSlowTree(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	for i := 0; i < 5000; i++ {
		name := filepath.Join(root, "f"+strconv.Itoa(i)+".txt")
		if err := os.WriteFile(name, []byte(strconv.Itoa(i)), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

// --- 1. Version() stays a string (clause: go.md "Version() string"). ---------

func TestVersionIsString(t *testing.T) {
	var v string = snapdir.Version() // compile-pins the return type is string
	if v == "" {
		t.Fatal("Version() returned empty string")
	}
}

// --- 2. nil options accepted == defaults (clause: nil opts everywhere). ------

func TestNilOptionsAccepted(t *testing.T) {
	root := buildTree(t)
	ctx := context.Background()

	noPanic(t, "Manifest(nil opts)", func() {
		m, err := snapdir.Manifest(ctx, root, nil)
		if err != nil {
			t.Fatalf("Manifest with nil opts must work (nil == defaults): %v", err)
		}
		if m == nil || len(m.Entries) == 0 {
			t.Fatal("Manifest(nil opts) returned no entries for a non-empty tree")
		}
	})

	noPanic(t, "ID(nil opts)", func() {
		id, err := snapdir.ID(ctx, root, nil)
		if err != nil {
			t.Fatalf("ID with nil opts must work (nil == defaults): %v", err)
		}
		if !hex64.MatchString(id) {
			t.Fatalf("ID(nil opts) not 64-lowercase-hex: %q", id)
		}
	})
}

// --- 3. No panic + typed error w/ stable .Code on a failing op. --------------
//
// Clause: "every failure is a returned *SnapdirError{Code,Message}; no panic";
// Code ∈ the 8 stable codes. A missing path is the canonical failure.

func TestMissingPathReturnsTypedErrorNoPanic(t *testing.T) {
	ctx := context.Background()

	noPanic(t, "Manifest(missing path)", func() {
		m, err := snapdir.Manifest(ctx, "/no/such/path/snapdir-xyz", nil)
		if err == nil {
			t.Fatal("Manifest on a missing path must return an error")
		}
		if m != nil {
			t.Fatalf("Manifest on failure must return a nil result, got %+v", m)
		}

		// error must carry a typed *SnapdirError extractable via errors.As.
		var se *snapdir.SnapdirError
		if !errors.As(err, &se) {
			t.Fatalf("error must be a *snapdir.SnapdirError, got %T: %v", err, err)
		}
		if !stableCodes[se.Code] && se.Code != "INTERNAL" {
			t.Fatalf("SnapdirError.Code %q is not one of the 8 stable codes", se.Code)
		}
		// IO failure on a missing path is the expected stable code.
		if se.Code != "IO_ERROR" {
			t.Logf("note: missing-path Code = %q (expected IO_ERROR)", se.Code)
		}
		if se.Message == "" {
			t.Fatal("SnapdirError.Message must be non-empty")
		}
		// SnapdirError implements the error interface.
		var _ error = se
		if se.Error() == "" {
			t.Fatal("SnapdirError.Error() must be non-empty")
		}
	})
}

// --- 4. context.Context cancellation: explicit cancel. -----------------------
//
// Clause (go.md headline): blocking C op runs to completion on a goroutine; an
// explicitly-cancelled ctx makes the CALL return context.Canceled.

func TestContextExplicitCancel(t *testing.T) {
	root := buildSlowTree(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // already cancelled before the call

	noPanic(t, "Manifest(cancelled ctx)", func() {
		_, err := snapdir.Manifest(ctx, root, nil)
		if err == nil {
			t.Fatal("Manifest with a pre-cancelled ctx must return an error")
		}
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("expected errors.Is(err, context.Canceled), got %v", err)
		}
	})
}

// --- 4b. context.Context cancellation: deadline exceeded. ---------------------
//
// Clause: a very short deadline on a SLOW op yields context.DeadlineExceeded.

func TestContextDeadlineExceeded(t *testing.T) {
	root := buildSlowTree(t)
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Millisecond)
	defer cancel()

	noPanic(t, "Manifest(deadline)", func() {
		_, err := snapdir.Manifest(ctx, root, nil)
		if err == nil {
			t.Fatal("Manifest with a 1ms deadline over a 5000-file tree must return an error")
		}
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("expected errors.Is(err, context.DeadlineExceeded), got %v", err)
		}
	})
}

// --- 5. result types: Size is uint64; entries parse the manifest columns. ----
//
// Clause: Manifest{Entries,Raw}; ManifestEntry{Path,PathType,Permissions uint32,
// Checksum,Size uint64}. Size MUST be uint64 (pinned by a type assertion + a
// value that exceeds the int32 range to prove the width is real).

func TestResultTypesAndUint64Size(t *testing.T) {
	root := t.TempDir()
	// > 4 GiB would be wasteful to write; instead pin the field WIDTH via a
	// compile-time type assertion, and a runtime large-value round-trip.
	if err := os.WriteFile(filepath.Join(root, "big.dat"), make([]byte, 1<<20), 0o644); err != nil {
		t.Fatal(err)
	}

	m, err := snapdir.Manifest(context.Background(), root, nil)
	if err != nil {
		t.Fatalf("Manifest: %v", err)
	}
	if m.Raw == "" {
		t.Fatal("Manifest.Raw must hold the raw manifest text")
	}

	var foundBig bool
	for _, e := range m.Entries {
		// COMPILE-PIN: Size is uint64 — assigning to a uint64 var must typecheck
		// with no conversion, and a uint64 can hold a value > math.MaxInt32.
		var sz uint64 = e.Size
		_ = sz

		// COMPILE-PIN: Permissions is uint32, Checksum is a 64-hex string.
		var perm uint32 = e.Permissions
		_ = perm
		if e.PathType == 'F' || e.PathType == 'D' || e.PathType == 'L' {
			// PathType is a typed byte over D/F/L (clause: go.md typed byte consts)
		} else {
			t.Fatalf("unexpected PathType %q for %q", e.PathType, e.Path)
		}
		if e.PathType == 'F' && !hex64.MatchString(e.Checksum) {
			t.Fatalf("file entry %q checksum not 64-hex: %q", e.Path, e.Checksum)
		}
		if e.Path == "" {
			t.Fatal("entry Path must be non-empty")
		}
		if filepath.Base(e.Path) == "big.dat" {
			foundBig = true
			// the 1 MiB file proves Size carries a real byte count.
			if e.Size != uint64(1<<20) {
				t.Fatalf("big.dat Size = %d, want %d", e.Size, 1<<20)
			}
			// uint64 must accept a value far beyond int32 without overflow.
			huge := e.Size + (uint64(1) << 40)
			if huge < e.Size {
				t.Fatal("Size is not really uint64 (overflowed adding 2^40)")
			}
		}
	}
	if !foundBig {
		t.Fatal("big.dat entry not found in manifest")
	}
}

// --- 6. id self-consistency black-box oracle. --------------------------------
//
// Clause: ID(ctx,path,nil) == IDFromManifest(Manifest(ctx,path,nil)); the id is
// 64-lowercase-hex; IDFromManifest is a pure/sync (no-ctx) function.

func TestIDSelfConsistency(t *testing.T) {
	root := buildTree(t)
	ctx := context.Background()

	id, err := snapdir.ID(ctx, root, nil)
	if err != nil {
		t.Fatalf("ID: %v", err)
	}
	if !hex64.MatchString(id) {
		t.Fatalf("ID not 64-lowercase-hex: %q", id)
	}

	m, err := snapdir.Manifest(ctx, root, nil)
	if err != nil {
		t.Fatalf("Manifest: %v", err)
	}

	// IDFromManifest is pure/sync — note: NO ctx argument (compile-pins it).
	idFromManifest, err := snapdir.IDFromManifest(m)
	if err != nil {
		t.Fatalf("IDFromManifest: %v", err)
	}
	if idFromManifest != id {
		t.Fatalf("id mismatch: ID()=%s IDFromManifest()=%s", id, idFromManifest)
	}

	// Determinism: a second ID() over the unchanged tree is identical.
	id2, err := snapdir.ID(ctx, root, nil)
	if err != nil {
		t.Fatalf("ID (re-run): %v", err)
	}
	if id2 != id {
		t.Fatalf("ID not deterministic: %s != %s", id, id2)
	}
}

// --- 6b. options actually take effect (Exclude changes the id). --------------
//
// Clause: *ManifestOptions fields (Exclude, NoFollow, Absolute) are honoured —
// excluding a file MUST change the snapshot id vs the default walk.

func TestManifestOptionsHonoured(t *testing.T) {
	root := buildTree(t)
	ctx := context.Background()

	base, err := snapdir.ID(ctx, root, nil)
	if err != nil {
		t.Fatalf("ID base: %v", err)
	}

	excluded, err := snapdir.ID(ctx, root, &snapdir.ManifestOptions{Exclude: []string{"a.txt"}})
	if err != nil {
		t.Fatalf("ID excluded: %v", err)
	}
	if excluded == base {
		t.Fatal("Exclude option had no effect on the snapshot id")
	}
}

// --- 7. Diff result type: DiffEntry{Status, Path}; empty self-diff. ----------
//
// Clause: DiffStatus ∈ {A,D,M,=}; Diff(ctx,fromURI,toURI). A self-diff (same
// store on both sides) yields an empty (no-change) diff. Uses file:// URIs only
// — no network. We push a snapshot to a local file store, then self-diff it.

func TestDiffSelfIsEmpty(t *testing.T) {
	root := buildTree(t)
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir
	ctx := context.Background()

	noPanic(t, "Push", func() {
		id, err := snapdir.Push(ctx, root, storeURI)
		if err != nil {
			t.Fatalf("Push to local file store: %v", err)
		}
		if !hex64.MatchString(id) {
			t.Fatalf("Push returned id not 64-hex: %q", id)
		}
	})

	noPanic(t, "Diff(self)", func() {
		entries, err := snapdir.Diff(ctx, storeURI, storeURI)
		if err != nil {
			t.Fatalf("self Diff: %v", err)
		}
		for _, e := range entries {
			switch e.Status {
			case 'A', 'D', 'M', '=':
			default:
				t.Fatalf("DiffEntry.Status %q not one of A/D/M/=", e.Status)
			}
			// a strict (non-unchanged) self-diff has no A/D/M rows.
			if e.Status != '=' {
				t.Fatalf("self-diff produced a change row %q %q, expected none", e.Status, e.Path)
			}
		}
	})
}

// --- 7b. Diff on an INVALID store URI returns a typed error, no panic. -------
//
// Re-addressed (PM, go-api-impl): a *missing but valid* file:// store is NOT an
// error — snapdir treats an absent store as empty (you can diff against a store
// before pushing to it), so Diff returns an empty result, no error. The
// typed-error contract is pinned on a GENUINELY invalid store URI (bad/unknown
// scheme → INVALID_STORE), which is what "a bogus store" must mean here.
func TestDiffBogusStoreTypedError(t *testing.T) {
	ctx := context.Background()
	noPanic(t, "Diff(invalid store URI)", func() {
		_, err := snapdir.Diff(ctx, "not-a-uri", "ftp://x/y")
		if err == nil {
			t.Fatal("Diff on an invalid store URI must return an error")
		}
		var se *snapdir.SnapdirError
		if !errors.As(err, &se) {
			t.Fatalf("Diff error must be *snapdir.SnapdirError, got %T", err)
		}
		if !stableCodes[se.Code] && se.Code != "INTERNAL" {
			t.Fatalf("Diff error Code %q not a stable code", se.Code)
		}
	})
	// A missing-but-valid file:// store must NOT panic and must NOT error
	// (it is an empty store → empty diff) — the binding faithfully reflects
	// snapdir's absent-store-is-empty semantics.
	noPanic(t, "Diff(missing file store)", func() {
		if _, err := snapdir.Diff(ctx, "file:///no/such/store/snapdir-xyz", "file:///also/missing"); err != nil {
			t.Fatalf("Diff on a missing file:// store must be empty (no error), got %v", err)
		}
	})
}

// --- 8. Pull signature compile-pin (no ctx-less variant). --------------------
//
// Clause: Pull(ctx, snapshotID, storeURI, dest) error — a missing snapshot id
// returns a typed error without panicking, never a partial materialization.

func TestPullMissingSnapshotTypedError(t *testing.T) {
	ctx := context.Background()
	dest := t.TempDir()
	storeURI := "file://" + t.TempDir() // empty store
	badID := "0000000000000000000000000000000000000000000000000000000000000000"

	noPanic(t, "Pull(missing id)", func() {
		err := snapdir.Pull(ctx, badID, storeURI, dest)
		if err == nil {
			t.Fatal("Pull of a non-existent snapshot must return an error")
		}
		var se *snapdir.SnapdirError
		if !errors.As(err, &se) {
			t.Fatalf("Pull error must be *snapdir.SnapdirError, got %T", err)
		}
		if !stableCodes[se.Code] && se.Code != "INTERNAL" {
			t.Fatalf("Pull error Code %q not a stable code", se.Code)
		}
	})
}

// --- STRENGTHENING (tests-review, adversary/opus via PM) ---------------------

// TestFileStoreRoundtripRestoresPerms exercises the shared snapdir-api
// permission-restore contract THROUGH the Go binding: push a tree (with
// non-default perms — sub/b.bin is 0600), then Pull into a PRE-EXISTING
// restrictive (0700) dest, and assert the materialized tree re-IDs to the pushed
// id. A pull that didn't restore each entry's mode would re-id differently.
func TestFileStoreRoundtripRestoresPerms(t *testing.T) {
	root := buildTree(t)
	ctx := context.Background()
	storeURI := "file://" + t.TempDir()

	pushed, err := snapdir.Push(ctx, root, storeURI)
	if err != nil {
		t.Fatalf("Push: %v", err)
	}
	local, err := snapdir.ID(ctx, root, nil)
	if err != nil {
		t.Fatalf("ID(root): %v", err)
	}
	if pushed != local {
		t.Fatalf("push id %q != local id %q (push must not mutate the manifest)", pushed, local)
	}

	dest := t.TempDir() // pre-existing dir; tighten it to 0700 to stress perm-restore
	if err := os.Chmod(dest, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := snapdir.Pull(ctx, pushed, storeURI, dest); err != nil {
		t.Fatalf("Pull: %v", err)
	}
	reid, err := snapdir.ID(ctx, dest, nil)
	if err != nil {
		t.Fatalf("ID(dest): %v", err)
	}
	if reid != pushed {
		t.Fatalf("pulled tree re-id %q != pushed id %q (permission-restore failed)", reid, pushed)
	}
}

// TestNoFollowOptionChangesID pins that the native no_follow option (the C ABI
// exposes it on snapdir_manifest) produces a DISTINCT id from the default follow
// walk on a tree containing a symlink — complementing the Exclude-only option
// test. (snapshot ids hash the manifest text, which differs when a symlink is
// recorded vs dereferenced.)
func TestNoFollowOptionChangesID(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "target.txt"), []byte("target\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("target.txt", filepath.Join(root, "link")); err != nil {
		t.Skipf("symlinks unsupported here: %v", err)
	}
	ctx := context.Background()

	follow, err := snapdir.ID(ctx, root, nil) // default follows
	if err != nil {
		t.Fatalf("ID follow: %v", err)
	}
	noFollow, err := snapdir.ID(ctx, root, &snapdir.ManifestOptions{NoFollow: true})
	if err != nil {
		t.Fatalf("ID no_follow: %v", err)
	}
	if !hex64.MatchString(follow) || !hex64.MatchString(noFollow) {
		t.Fatalf("ids not 64-hex: %q %q", follow, noFollow)
	}
	if follow == noFollow {
		t.Fatal("no_follow produced the same id as the default follow walk")
	}
}
