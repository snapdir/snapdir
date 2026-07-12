// Black-box spec for the snapdir Go binding's `snapdir size` surface
// (Phase 46, go-size-spec-tests, adversary/opus — AUTHORING mode).
//
// Authored from the SPEC + the Go binding's public conventions ONLY. The author
// did NOT read the Rust ffi/api `src/`; the field order/names for SizeStats were
// confirmed against the generated C header (include/snapdir.h::SnapdirSizeStats).
// snapdir.Size / snapdir.ManifestSize / snapdir.SizeStats do NOT exist yet — this
// file is EXPECTED to not compile until the impl gate adds them. Do not weaken.
//
// Pinned contract surface (the impl MUST satisfy these signatures/shapes):
//
//	type SizeStats struct {
//		LogicalBytes  uint64  // Σ size over ALL File entries (dups counted)
//		PhysicalBytes uint64  // Σ size over UNIQUE checksums (deduped == .objects/ bytes)
//		Files         uint64  // count of File entries
//		Objects       uint64  // count of distinct checksums
//	}
//	func ManifestSize(m *ManifestResult) SizeStats                       // pure/sync, no ctx, no error
//	func Size(ctx context.Context, storeURI, id string) (SizeStats, error) // id=="" ⇒ whole store
//
// BigQuery nomenclature: objects are stored UNCOMPRESSED, so PhysicalBytes MUST
// equal the real on-disk `<store>/.objects/` byte total for a file:// store — the
// acceptance bar, asserted by walking `.objects/` with filepath.WalkDir.
//
// This file lives in `package snapdir_test` alongside snapdir_api_test.go and
// REUSES its shared helpers (stableCodes, hex64, noPanic) — they are intentionally
// NOT redeclared here to avoid duplicate-declaration errors once this file is
// `git mv`d into bindings/go/.
package snapdir_test

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"

	snapdir "github.com/snapdir/snapdir/bindings/go"
)

// Deterministic fixture — MATCHES the Rust/cpp size tests so cross-binding parity
// holds. seed/b/dup.txt is a byte-for-byte DUPLICATE of seed/a/x.txt (same
// checksum), so it collapses under dedup:
//
//	seed/a/x.txt   = "hello world\n"      (12 bytes)
//	seed/b/dup.txt = "hello world\n"      (12 bytes — DUP of x.txt)
//	seed/a/y.txt   = "unique data here\n" (17 bytes)
//
// ⇒ Files=3, Objects=2, LogicalBytes=41 (12+12+17), PhysicalBytes=29 (12+17).
const (
	wantFiles    = uint64(3)
	wantObjects  = uint64(2)
	wantLogical  = uint64(41)
	wantPhysical = uint64(29)
)

// buildSizeSeed writes the deterministic dedup fixture and returns its root.
// Uniquely named so it does not collide with buildTree in snapdir_api_test.go.
func buildSizeSeed(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "a"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, "b"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a", "x.txt"), []byte("hello world\n"), 0o644); err != nil {
		t.Fatal(err) // 12 bytes
	}
	if err := os.WriteFile(filepath.Join(root, "b", "dup.txt"), []byte("hello world\n"), 0o644); err != nil {
		t.Fatal(err) // 12 bytes — DUPLICATE content of a/x.txt
	}
	if err := os.WriteFile(filepath.Join(root, "a", "y.txt"), []byte("unique data here\n"), 0o644); err != nil {
		t.Fatal(err) // 17 bytes
	}
	return root
}

// sumObjectsDir walks <storeDir>/.objects and sums the byte size of every regular
// file — the ground-truth on-disk footprint. Because snapdir stores objects
// UNCOMPRESSED, this MUST equal SizeStats.PhysicalBytes.
func sumObjectsDir(t *testing.T, storeDir string) uint64 {
	t.Helper()
	objectsDir := filepath.Join(storeDir, ".objects")
	var total uint64
	err := filepath.WalkDir(objectsDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		info, ierr := d.Info()
		if ierr != nil {
			return ierr
		}
		total += uint64(info.Size())
		return nil
	})
	if err != nil {
		t.Fatalf("walking %s: %v", objectsDir, err)
	}
	return total
}

// mustManifestSize parses the seed manifest and returns its ManifestSize.
func mustManifestSize(t *testing.T, seed string) snapdir.SizeStats {
	t.Helper()
	m, err := snapdir.Manifest(context.Background(), seed, nil)
	if err != nil {
		t.Fatalf("Manifest(seed): %v", err)
	}
	// ManifestSize is pure/sync — NO ctx argument (compile-pins the signature).
	return snapdir.ManifestSize(m)
}

// --- 1. ManifestSize dedup-by-checksum over a LOCAL manifest. -----------------
//
// Clause: ManifestSize(Manifest(ctx,seed,nil)) == {41,29,3,2}. LogicalBytes counts
// dups; PhysicalBytes dedups by checksum; the manifest alone is authoritative
// (objects are uncompressed) so no store round-trip is needed.
func TestManifestSizeDedupOverLocalManifest(t *testing.T) {
	seed := buildSizeSeed(t)
	var got snapdir.SizeStats
	noPanic(t, "ManifestSize", func() {
		got = mustManifestSize(t, seed)
	})
	// SizeStats field WIDTHS are pinned by uint64 assignment (compile-time).
	var _ uint64 = got.LogicalBytes
	var _ uint64 = got.PhysicalBytes
	var _ uint64 = got.Files
	var _ uint64 = got.Objects

	want := snapdir.SizeStats{LogicalBytes: wantLogical, PhysicalBytes: wantPhysical, Files: wantFiles, Objects: wantObjects}
	if got != want { // struct equality pins every field at once
		t.Fatalf("ManifestSize = %+v, want %+v", got, want)
	}
}

// --- 2. Size(store, id) over a fresh file:// store == ManifestSize figure. -----
//
// Clause: pushing the seed and asking Size for the PUSH'S RETURNED id yields the
// exact same SizeStats as ManifestSize over the local tree — store size reconciles
// with local size.
func TestSizeByIDMatchesManifestSize(t *testing.T) {
	seed := buildSizeSeed(t)
	ctx := context.Background()
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir

	id, err := snapdir.Push(ctx, seed, storeURI)
	if err != nil {
		t.Fatalf("Push: %v", err)
	}
	if !hex64.MatchString(id) {
		t.Fatalf("Push returned id not 64-hex: %q", id)
	}

	want := mustManifestSize(t, seed)
	var got snapdir.SizeStats
	noPanic(t, "Size(ctx,store,id)", func() {
		got, err = snapdir.Size(ctx, storeURI, id)
		if err != nil {
			t.Fatalf("Size(id): %v", err)
		}
	})
	if got != want { // by-id store size must equal local ManifestSize exactly
		t.Fatalf("Size(store,id) = %+v, want ManifestSize %+v", got, want)
	}
}

// --- 3. Size(store, "") whole store (one snapshot) == single-snapshot figure. --
//
// Clause: id=="" ⇒ whole store, all snapshots deduped. With exactly one snapshot
// pushed, the whole-store figure equals the single-snapshot figure, and
// PhysicalBytes is 29 absolutely.
func TestSizeWholeStoreSingleSnapshot(t *testing.T) {
	seed := buildSizeSeed(t)
	ctx := context.Background()
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir

	if _, err := snapdir.Push(ctx, seed, storeURI); err != nil {
		t.Fatalf("Push: %v", err)
	}

	var got snapdir.SizeStats
	noPanic(t, `Size(ctx,store,"")`, func() {
		var err error
		got, err = snapdir.Size(ctx, storeURI, "") // "" ⇒ whole store
		if err != nil {
			t.Fatalf(`Size(store,""): %v`, err)
		}
	})
	want := snapdir.SizeStats{LogicalBytes: wantLogical, PhysicalBytes: wantPhysical, Files: wantFiles, Objects: wantObjects}
	if got != want {
		t.Fatalf(`Size(store,"") = %+v, want %+v`, got, want)
	}
	if got.PhysicalBytes != wantPhysical { // 29 absolutely, no dedup slop
		t.Fatalf("whole-store PhysicalBytes = %d, want %d", got.PhysicalBytes, wantPhysical)
	}
}

// --- 4. ACCEPTANCE: PhysicalBytes == on-disk .objects/ byte total == 29. -------
//
// Clause (THE acceptance bar): objects are stored UNCOMPRESSED, so the deduped
// PhysicalBytes reported by Size MUST equal the real byte sum of <store>/.objects/
// obtained by walking the directory, and that sum MUST be 29.
func TestSizePhysicalBytesEqualsOnDiskObjects(t *testing.T) {
	seed := buildSizeSeed(t)
	ctx := context.Background()
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir

	if _, err := snapdir.Push(ctx, seed, storeURI); err != nil {
		t.Fatalf("Push: %v", err)
	}

	stats, err := snapdir.Size(ctx, storeURI, "")
	if err != nil {
		t.Fatalf(`Size(store,""): %v`, err)
	}
	onDisk := sumObjectsDir(t, storeDir)

	if stats.PhysicalBytes != onDisk { // uncompressed ⇒ reported == on-disk
		t.Fatalf("PhysicalBytes = %d but .objects/ on-disk total = %d (objects are uncompressed, must match)", stats.PhysicalBytes, onDisk)
	}
	if onDisk != wantPhysical { // and the on-disk truth is exactly 29
		t.Fatalf(".objects/ on-disk total = %d, want %d", onDisk, wantPhysical)
	}
}

// --- 5. Strict dedup relations: LogicalBytes > PhysicalBytes, Objects < Files. -
//
// Clause: the fixture contains a genuine duplicate, so dedup MUST actually
// collapse it — apparent size strictly exceeds stored size, and there are strictly
// fewer distinct objects than file entries.
func TestSizeDedupStrictInequalities(t *testing.T) {
	seed := buildSizeSeed(t)
	ctx := context.Background()
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir

	if _, err := snapdir.Push(ctx, seed, storeURI); err != nil {
		t.Fatalf("Push: %v", err)
	}
	stats, err := snapdir.Size(ctx, storeURI, "")
	if err != nil {
		t.Fatalf(`Size(store,""): %v`, err)
	}
	if !(stats.LogicalBytes > stats.PhysicalBytes) { // dedup saved bytes
		t.Fatalf("expected LogicalBytes(%d) > PhysicalBytes(%d)", stats.LogicalBytes, stats.PhysicalBytes)
	}
	if !(stats.Objects < stats.Files) { // dedup collapsed a duplicate entry
		t.Fatalf("expected Objects(%d) < Files(%d)", stats.Objects, stats.Files)
	}
}

// --- 6. Bad/unopenable store ⇒ typed *SnapdirError, no panic. ------------------
//
// Clause: Size on a bad/unopenable store returns a non-nil error extractable via
// errors.As into *snapdir.SnapdirError, with a Code among the 8 stable codes (or
// "INTERNAL") and a non-empty Message. The binding must NOT panic.
func TestSizeBadStoreTypedErrorNoPanic(t *testing.T) {
	ctx := context.Background()
	noPanic(t, "Size(bad store)", func() {
		// An absent file:// store ERRORS for Size (STORE_ERROR "store location does
		// not exist") — unlike Diff, Size does not treat a missing store as empty
		// (see TestSizeAbsentStoreErrors / case 9). The original absent-path URI is
		// therefore a valid bad-store input and is used here to prove it.
		_, err := snapdir.Size(ctx, "file:///nonexistent/deep/missing", "")
		if err == nil {
			t.Fatal("Size on a bad/unopenable store must return an error")
		}
		var se *snapdir.SnapdirError
		if !errors.As(err, &se) {
			t.Fatalf("Size error must be *snapdir.SnapdirError, got %T: %v", err, err)
		}
		if !stableCodes[se.Code] && se.Code != "INTERNAL" {
			t.Fatalf("Size error Code %q is not one of the 8 stable codes", se.Code)
		}
		if se.Message == "" {
			t.Fatal("SnapdirError.Message must be non-empty")
		}
	})
}

// --- 7. Context cancellation honoured (pre-cancelled ctx). --------------------
//
// Clause (mirrors TestContextExplicitCancel): a ctx cancelled BEFORE the call
// makes Size return a non-nil error surfacing the cancellation — either
// context.Canceled or a typed *SnapdirError — without hanging or panicking.
func TestSizeContextCancelHonoured(t *testing.T) {
	seed := buildSizeSeed(t)
	storeDir := t.TempDir()
	storeURI := "file://" + storeDir
	if _, err := snapdir.Push(ctx0(), seed, storeURI); err != nil {
		t.Fatalf("Push: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancelled before the call

	noPanic(t, "Size(cancelled ctx)", func() {
		_, err := snapdir.Size(ctx, storeURI, "")
		if err == nil {
			t.Fatal("Size with a pre-cancelled ctx must return an error")
		}
		// Accept either the stdlib sentinel or a typed binding error.
		var se *snapdir.SnapdirError
		if !errors.Is(err, context.Canceled) && !errors.As(err, &se) {
			t.Fatalf("cancelled Size must surface context.Canceled or *snapdir.SnapdirError, got %T: %v", err, err)
		}
	})
}

// ctx0 is a tiny helper so the Push setup in the cancel test uses a live context
// (the cancelled ctx is only for the Size call under test).
func ctx0() context.Context { return context.Background() }

// --- 8. Degenerate: ManifestSize of a File-less manifest ⇒ all zeros. ----------
//
// Clause: a manifest with only directory entries (no File entries) yields
// SizeStats{0,0,0,0} with no panic — the empty/degenerate boundary.
func TestManifestSizeNoFilesIsZero(t *testing.T) {
	root := t.TempDir()
	// Only directories, no files ⇒ the manifest has zero File entries.
	if err := os.MkdirAll(filepath.Join(root, "empty", "nested"), 0o755); err != nil {
		t.Fatal(err)
	}
	m, err := snapdir.Manifest(context.Background(), root, nil)
	if err != nil {
		t.Fatalf("Manifest(dirs-only): %v", err)
	}
	var got snapdir.SizeStats
	noPanic(t, "ManifestSize(no files)", func() {
		got = snapdir.ManifestSize(m)
	})
	want := snapdir.SizeStats{} // all zeros
	if got != want {
		t.Fatalf("ManifestSize over a File-less manifest = %+v, want all-zeros %+v", got, want)
	}
}

// --- HARDENING (go-size-tests-review, adversary/opus via PM) ------------------

// --- 9. Absent (missing-root) file:// store => typed STORE_ERROR: the TRUE -----
//
//	flip side of case 6.
//
// NOTE (go-size-tests-review, adversary/opus): the reviewer's case-6 rationale
// (and this gate's brief) claimed an absent file:// path is "treated as empty
// (no error) for Size". That is FALSE for Size -- it is Diff's behavior.
// Verified against source:
//   - snapdir-api size_sync PROPAGATES list_manifest_ids() errors
//     (crates/snapdir-api/src/lib.rs:1632, `.map_err(SnapdirError::from)?`);
//   - snapdir-api diff SWALLOWS them (src/lib.rs:1790, `let Ok(ids) = ... else {continue}`);
//   - snapdir-stores file_store DELIBERATELY errors when the store ROOT is
//     missing (src/file_store.rs:868, "store location does not exist") so an
//     absent store cannot masquerade as empty and fabricate a deletion delta.
//
// This is intended, documented behavior -- NOT an impl bug -- so we pin the REAL
// contract: a missing-root file:// store errors (STORE_ERROR), typed and
// panic-free, complementing case 6's bad-SCHEME => INVALID_STORE.
func TestSizeAbsentStoreErrors(t *testing.T) {
	ctx := context.Background()
	// A file:// path whose ROOT does NOT exist on disk.
	missing := "file://" + filepath.Join(t.TempDir(), "no-store-here")
	noPanic(t, "Size(absent store)", func() {
		_, err := snapdir.Size(ctx, missing, "")
		if err == nil { // clause: a missing-root file:// store errors (NOT empty -- that is Diff's semantics)
			t.Fatal("Size on an absent (missing-root) file:// store must return an error")
		}
		var se *snapdir.SnapdirError
		if !errors.As(err, &se) { // clause: typed *snapdir.SnapdirError, extractable via errors.As
			t.Fatalf("Size error must be *snapdir.SnapdirError, got %T: %v", err, err)
		}
		if !stableCodes[se.Code] && se.Code != "INTERNAL" { // clause: Code is one of the 8 stable codes (STORE_ERROR here)
			t.Fatalf("Size error Code %q is not one of the 8 stable codes", se.Code)
		}
		if se.Message == "" { // clause: SnapdirError.Message must be non-empty
			t.Fatal("SnapdirError.Message must be non-empty")
		}
	})
}

// --- 10. Two-snapshot cross-snapshot dedup with COMPUTED references. -----------
//
// Two snapshots pushed to ONE store share the 12-byte "hello world\n" object.
// Whole-store physical/object counts MUST collapse that shared object across
// snapshots, while logical bytes and file counts stay additive. Every reference
// is COMPUTED from the local manifests / on-disk .objects — no hardcoded sizes.
func TestSizeCrossSnapshotDedup(t *testing.T) {
	ctx := context.Background()

	// Seed A: the deterministic dedup fixture (Files=3, Objects=2), contains the
	// 12-byte "hello world\n" object.
	seedA := buildSizeSeed(t)

	// Seed B: a fresh tree sharing A's 12-byte object plus one brand-new object.
	seedB := t.TempDir()
	if err := os.WriteFile(filepath.Join(seedB, "shared.txt"), []byte("hello world\n"), 0o644); err != nil {
		t.Fatal(err) // 12 bytes — SAME content as seedA's a/x.txt (shared object)
	}
	if err := os.WriteFile(filepath.Join(seedB, "fresh.txt"), []byte("brand new content!\n"), 0o644); err != nil {
		t.Fatal(err) // a NEW object unique to seed B
	}

	// One store, two pushes.
	storeDir := t.TempDir()
	store := "file://" + storeDir
	idA, err := snapdir.Push(ctx, seedA, store)
	if err != nil {
		t.Fatalf("Push(seedA): %v", err)
	}
	idB, err := snapdir.Push(ctx, seedB, store)
	if err != nil {
		t.Fatalf("Push(seedB): %v", err)
	}

	// COMPUTED references from the LOCAL manifests (reuse mustManifestSize).
	aStats := mustManifestSize(t, seedA)
	bStats := mustManifestSize(t, seedB)

	// Per-id selection: an id addresses its OWN snapshot, not the whole store.
	gotA, err := snapdir.Size(ctx, store, idA)
	if err != nil {
		t.Fatalf("Size(store,idA): %v", err)
	}
	if gotA != aStats { // clause: id selects the specific snapshot, not whole store
		t.Fatalf("Size(store,idA) = %+v, want aStats %+v", gotA, aStats)
	}
	gotB, err := snapdir.Size(ctx, store, idB)
	if err != nil {
		t.Fatalf("Size(store,idB): %v", err)
	}
	if gotB != bStats { // clause: id selects the specific snapshot, not whole store
		t.Fatalf("Size(store,idB) = %+v, want bStats %+v", gotB, bStats)
	}
	if idA == idB { // clause: the two distinct trees have distinct snapshot ids
		t.Fatalf("expected distinct snapshot ids, both = %q", idA)
	}

	// Whole store: id=="" folds all snapshots, deduped across snapshots.
	whole, err := snapdir.Size(ctx, store, "")
	if err != nil {
		t.Fatalf(`Size(store,""): %v`, err)
	}
	if whole.Files != aStats.Files+bStats.Files { // clause: whole-store files = union of all File entries (dups counted)
		t.Fatalf("whole.Files = %d, want %d (aStats %d + bStats %d)", whole.Files, aStats.Files+bStats.Files, aStats.Files, bStats.Files)
	}
	if whole.LogicalBytes != aStats.LogicalBytes+bStats.LogicalBytes { // clause: logical is additive across snapshots
		t.Fatalf("whole.LogicalBytes = %d, want %d (aStats %d + bStats %d)", whole.LogicalBytes, aStats.LogicalBytes+bStats.LogicalBytes, aStats.LogicalBytes, bStats.LogicalBytes)
	}
	if !(whole.PhysicalBytes < aStats.PhysicalBytes+bStats.PhysicalBytes) { // clause: STRICT — the shared 12-byte object is deduped ACROSS snapshots
		t.Fatalf("expected whole.PhysicalBytes(%d) < aStats.PhysicalBytes(%d)+bStats.PhysicalBytes(%d)=%d (cross-snapshot dedup)", whole.PhysicalBytes, aStats.PhysicalBytes, bStats.PhysicalBytes, aStats.PhysicalBytes+bStats.PhysicalBytes)
	}
	if !(whole.Objects < aStats.Objects+bStats.Objects) { // clause: distinct-object count collapses the shared object
		t.Fatalf("expected whole.Objects(%d) < aStats.Objects(%d)+bStats.Objects(%d)=%d (shared object collapsed)", whole.Objects, aStats.Objects, bStats.Objects, aStats.Objects+bStats.Objects)
	}

	// Acceptance: whole-store physical == the real on-disk uncompressed .objects/ total.
	onDisk := sumObjectsDir(t, storeDir)
	if whole.PhysicalBytes != onDisk { // clause: multi-snapshot acceptance: physical == on-disk uncompressed bytes
		t.Fatalf("whole.PhysicalBytes = %d but .objects/ on-disk total = %d (uncompressed, must match)", whole.PhysicalBytes, onDisk)
	}

	// Determinism: a second whole-store query is identical.
	whole2, err := snapdir.Size(ctx, store, "")
	if err != nil {
		t.Fatalf(`Size(store,"") re-run: %v`, err)
	}
	if whole2 != whole { // clause: deterministic
		t.Fatalf("whole-store Size not deterministic: %+v != %+v", whole2, whole)
	}
}
