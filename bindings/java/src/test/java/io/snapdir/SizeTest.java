// SizeTest.java — black-box spec for the snapdir `size` surface of the Java
// (JDK Foreign Function) binding (Phase 46, gate java-size-spec-tests;
// adversary/opus). I did NOT write the impl.
//
// Authored from the SPEC ONLY + the PUBLIC io.snapdir surface
// (Snapdir/Manifest/ManifestEntry/SnapdirException/ManifestOptions/PathType)
// + the FROZEN C ABI `SnapdirSizeStats` field ORDER from include/snapdir.h
// (logical_bytes, physical_bytes, files, objects — all uint64). I did NOT read
// crates/snapdir-ffi/src/ nor crates/snapdir-api/src/ (Rust). It is a
// self-contained plain-assert harness with public static void main(String[])
// (the offline image ships NO JUnit / maven / gradle — only openjdk-17 +
// javac/jar), modelled EXACTLY on SnapdirApiTest.java: a check(boolean,String)
// that counts failures and keeps going, and System.exit(failures==0?0:1).
//
// The Snapdir.size / Snapdir.manifestSize / SizeStats symbols do NOT exist yet
// — this is EXPECTED to not-yet-compile until the java-size-impl gate adds:
//   record SizeStats(long logicalBytes, long physicalBytes, long files, long objects)
//                                   + String physicalBytesUnsigned()   // == Long.toUnsignedString(physicalBytes)
//   static SizeStats                Snapdir.manifestSize(Manifest m);          // sync, no throw
//   static CompletableFuture<SizeStats> Snapdir.size(String storeUri, String id); // id==null ⇒ whole store
// Do NOT weaken these assertions to pass against the scaffold — it is CORRECT
// that it cannot compile until the impl lands.
//
// Semantics pinned (objects stored UNCOMPRESSED, so the manifest is authoritative
// and physicalBytes equals the on-disk `.objects/` byte total):
//   logicalBytes  = Σ size over ALL File entries (dups counted)
//   physicalBytes = Σ size over UNIQUE checksums
//   files         = count of File entries
//   objects       = count of distinct checksums
//
// Deterministic fixture (parity with cpp/go/Rust):
//   seed/a/x.txt   = "hello world\n"      (12)
//   seed/b/dup.txt = "hello world\n"      (12, dup of x.txt)
//   seed/a/y.txt   = "unique data here\n" (17)
//   ⇒ files=3, objects=2, logicalBytes=41, physicalBytes=29.

package io.snapdir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.stream.Stream;

/**
 * Self-contained black-box spec for the public {@code snapdir size} surface of
 * the {@code io.snapdir} binding. Lives in the {@code io.snapdir} package so it
 * can compile-pin the {@link SizeStats} record shape directly, but it exercises
 * ONLY the public static API. It hard-pins the uncompressed-object size
 * contract, the whole-store dedup figure, the ACCEPTANCE bar (physicalBytes ==
 * real {@code .objects/} bytes), and the three source-confirmed store-error
 * cases (missing-root ⇒ STORE_ERROR, existing-empty ⇒ zeros, bad-scheme ⇒
 * INVALID_STORE).
 */
public final class SizeTest {

    private SizeTest() {}

    // ---------------------------------------------------------------- tiny assert harness (counts failures, keeps going) ----------------------------------------------------------------

    private static int checks = 0;
    private static int failures = 0;

    private static void check(boolean cond, String msg) {
        checks++;
        if (!cond) {
            failures++;
            System.err.println("FAIL: " + msg);
        }
    }

    // ---------------------------------------------------------------- helpers (own private copies; separate class) ----------------------------------------------------------------

    // The deterministic seed tree (parity with cpp/go/Rust). Two files share the
    // "hello world\n" bytes (dup ⇒ one object); one file is unique.
    private static Path buildSeed(Path root) throws IOException {
        Files.createDirectories(root.resolve("a"));
        Files.createDirectories(root.resolve("b"));
        Files.writeString(root.resolve("a").resolve("x.txt"), "hello world\n");      // 12
        Files.writeString(root.resolve("b").resolve("dup.txt"), "hello world\n");    // 12 (dup)
        Files.writeString(root.resolve("a").resolve("y.txt"), "unique data here\n"); // 17
        return root;
    }

    // "file://" + absolute path — the file:// store URI convention (mirrors
    // SnapdirApiTest.fileStore).
    private static String storeUri(Path dir) {
        return "file://" + dir.toAbsolutePath();
    }

    // Walk the cause chain and return the first SnapdirException — CompletableFuture.get()
    // throws ExecutionException whose cause is the binding's SnapdirException,
    // possibly wrapped one extra layer in a CompletionException. Mirrors
    // SnapdirApiTest.findSnapdirCause. Returns null if none in the chain.
    private static SnapdirException findSnapdirCause(Throwable t) {
        Throwable cur = t;
        Set<Throwable> seen = new HashSet<>();
        while (cur != null && seen.add(cur)) {
            if (cur instanceof SnapdirException se) return se;
            cur = cur.getCause();
        }
        return null;
    }

    // The real on-disk footprint of a file:// store: Σ Files.size over every
    // regular file under <store>/.objects/. This is the ACCEPTANCE oracle for
    // physicalBytes (objects are uncompressed). Returns -1 if .objects/ absent.
    private static long objectsBytesOnDisk(Path storeDir) throws IOException {
        Path objects = storeDir.resolve(".objects");
        if (!Files.isDirectory(objects)) return -1L;
        try (Stream<Path> s = Files.walk(objects)) {
            return s.filter(Files::isRegularFile).mapToLong(p -> {
                try { return Files.size(p); } catch (IOException e) { return 0L; }
            }).sum();
        }
    }

    // Convenience: assert a SizeStats equals the fixture figure {41,29,3,2} by field.
    private static void checkFigure(SizeStats st, String where) {
        check(st != null, where + ": SizeStats must be non-null");
        if (st == null) return;
        check(st.logicalBytes() == 41L,  where + ": logicalBytes must be 41, got " + st.logicalBytes());
        check(st.physicalBytes() == 29L, where + ": physicalBytes must be 29, got " + st.physicalBytes());
        check(st.files() == 3L,          where + ": files must be 3, got " + st.files());
        check(st.objects() == 2L,        where + ": objects must be 2, got " + st.objects());
    }

    // ================================================================ 1. manifestSize over the parsed seed manifest == {41,29,3,2} ================================================================
    //
    // clause: manifestSize(manifest(seed)) == {logical=41, physical=29, files=3, objects=2}
    // — pure over the parsed manifest; the dup file counts toward logicalBytes/files
    // but NOT toward physicalBytes/objects (deduped by checksum).

    private static void testManifestSizeFigure(Path seed) throws Exception {
        Manifest m = Snapdir.manifest(seed.toString(), ManifestOptions.builder().build());
        check(m != null && !m.entries().isEmpty(), "manifest(seed) must be non-empty");
        // COMPILE-PIN: manifestSize is a sync static returning SizeStats (NO throws).
        SizeStats st = Snapdir.manifestSize(m);
        checkFigure(st, "manifestSize(manifest(seed))");
    }

    // ================================================================ 2. push(seed) → fresh file:// store; size(store, pushedId) == the figure ================================================================
    //
    // clause: size(store, <pushedId>).get() reproduces the manifestSize figure for
    // exactly the pushed snapshot.

    private static void testSizeOfPushedSnapshot(Path seed, Path storeDir) throws Exception {
        String store = storeUri(storeDir);
        // push() returns CompletableFuture<String> (the 64-hex snapshot id).
        String pushedId = Snapdir.push(seed.toString(), store, PushOptions.builder().build()).get();
        check(pushedId != null && pushedId.matches("^[0-9a-f]{64}$"),
            "push(seed) must return a 64-hex id, got " + pushedId);

        // COMPILE-PIN: size returns CompletableFuture<SizeStats>; .get() yields SizeStats.
        SizeStats st = Snapdir.size(store, pushedId).get();
        checkFigure(st, "size(store, pushedId)");

        // The single-snapshot size MUST equal the pure manifestSize figure (record equals).
        SizeStats fromManifest =
            Snapdir.manifestSize(Snapdir.manifest(seed.toString(), ManifestOptions.builder().build()));
        check(st != null && st.equals(fromManifest),
            "size(store, pushedId) must equal manifestSize(manifest(seed)): " + st + " vs " + fromManifest);
    }

    // ================================================================ 3. size(store, null) — whole store, 1 snapshot == the single figure ================================================================
    //
    // clause: size(store, null).get() (whole store, deduped across all snapshots;
    // here a single snapshot) == the single figure; physicalBytes == 29 absolutely.

    private static void testWholeStoreSize(Path storeDir) throws Exception {
        String store = storeUri(storeDir);
        SizeStats st = Snapdir.size(store, null).get();
        checkFigure(st, "size(store, null)");
        check(st != null && st.physicalBytes() == 29L,
            "whole-store physicalBytes must be exactly 29, got " + (st == null ? "<null>" : st.physicalBytes()));
    }

    // ================================================================ 4. ACCEPTANCE: physicalBytes == real .objects/ byte total == 29 ================================================================
    //
    // clause: size(store, null).get().physicalBytes() == Files.walk byte sum of
    // <store>/.objects/, and == 29. Objects are stored UNCOMPRESSED, so the
    // deduped logical footprint MUST match the on-disk object bytes exactly.

    private static void testPhysicalBytesMatchDisk(Path storeDir) throws Exception {
        String store = storeUri(storeDir);
        long onDisk = objectsBytesOnDisk(storeDir);
        check(onDisk >= 0L, "the pushed store must have a <store>/.objects/ directory to weigh");
        SizeStats st = Snapdir.size(store, null).get();
        check(st != null && onDisk == 29L,
            "on-disk .objects/ bytes must be 29 (uncompressed dedup), got " + onDisk);
        check(st != null && st.physicalBytes() == onDisk,
            "physicalBytes must EQUAL the real .objects/ byte total: "
                + (st == null ? "<null>" : st.physicalBytes()) + " vs " + onDisk);
    }

    // ================================================================ 5. strict inequalities: logicalBytes > physicalBytes AND objects < files ================================================================
    //
    // clause: the dup file guarantees a strict gap — logicalBytes (41) > physicalBytes
    // (29) and objects (2) < files (3). A no-dedup impl would collapse these to equal.

    private static void testStrictInequalities(Path seed) throws Exception {
        SizeStats st =
            Snapdir.manifestSize(Snapdir.manifest(seed.toString(), ManifestOptions.builder().build()));
        check(st != null && Long.compareUnsigned(st.logicalBytes(), st.physicalBytes()) > 0,
            "logicalBytes must be strictly greater than physicalBytes (dedup gap)");
        check(st != null && Long.compareUnsigned(st.objects(), st.files()) < 0,
            "objects must be strictly fewer than files (a dup was deduped)");
    }

    // ================================================================ 6. STORE-ERROR contract (a)/(b)/(c) — source-confirmed, pin exactly ================================================================
    //
    // (a) MISSING-root file:// store → size(...).get() throws; cause is a
    //     SnapdirException with getCode() == "STORE_ERROR" (a missing root does NOT
    //     masquerade as empty — that would fabricate a deletion delta). THE
    //     IMPORTANT CASE.
    // (b) EXISTING store dir with NO manifests → size(store, null).get() returns
    //     all-zero SizeStats, NO throw.
    // (c) malformed / unknown-scheme URI → throws; cause SnapdirException with
    //     getCode() == "INVALID_STORE".

    private static void testStoreErrorContract(Path root) throws Exception {
        // (a) a file:// path whose directory does NOT exist. NOT empty, NOT zeros.
        Path missing = root.resolve("no-such-store-zzz-missing");
        check(!Files.exists(missing), "precondition: the missing-root store dir must not exist");
        boolean threwMissing = false;
        try {
            SizeStats s = Snapdir.size(storeUri(missing), null).get();
            check(false, "size() on a MISSING-root file:// store must THROW, not return " + s);
        } catch (ExecutionException ee) {
            threwMissing = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null,
                "missing-root size().get() must wrap a SnapdirException in its cause chain, got " + ee.getCause());
            check(c != null && "STORE_ERROR".equals(c.getCode()),
                "missing-root store must be EXACTLY STORE_ERROR (not empty, not zeros), got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(threwMissing, "size() on a missing-root file:// store MUST throw from .get()");

        // (b) an EXISTING store dir with no manifests pushed → all-zero, NO throw.
        Path emptyStore = root.resolve("existing-empty-store");
        Files.createDirectories(emptyStore);
        SizeStats zero = Snapdir.size(storeUri(emptyStore), null).get();
        check(zero != null, "size() on an existing empty store must return non-null SizeStats");
        if (zero != null) {
            check(zero.logicalBytes() == 0L && zero.physicalBytes() == 0L
                    && zero.files() == 0L && zero.objects() == 0L,
                "an existing store with no manifests must be all-zero, got " + zero);
        }

        // (c) malformed / unknown-scheme URI → throws INVALID_STORE.
        boolean threwBogus = false;
        try {
            SizeStats s = Snapdir.size("bogus://nope", null).get();
            check(false, "size() on an unknown-scheme URI must THROW, not return " + s);
        } catch (ExecutionException ee) {
            threwBogus = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null,
                "bogus-scheme size().get() must wrap a SnapdirException, got " + ee.getCause());
            check(c != null && "INVALID_STORE".equals(c.getCode()),
                "an unknown-scheme URI must be EXACTLY INVALID_STORE, got "
                    + (c == null ? "<none>" : c.getCode()));
            check(c == null || !(((Exception) c) instanceof RuntimeException),
                "the wrapped cause must be a CHECKED SnapdirException, not a RuntimeException");
        }
        check(threwBogus, "size() on an unknown-scheme URI MUST throw from .get()");
    }

    // ================================================================ 7. unsigned accessor mirrors sizeUnsigned() convention ================================================================
    //
    // clause: SizeStats fields are `long` but logically UNSIGNED u64 (mirror of
    // SnapdirSizeStats). Mirroring ManifestEntry.sizeUnsigned() ==
    // Long.toUnsignedString(size), SizeStats MUST expose physicalBytesUnsigned()
    // == Long.toUnsignedString(physicalBytes). For the small fixture value the
    // unsigned decimal equals the signed one ("29"), but the accessor MUST exist
    // and be correct (the reason it exists is u64 values > Long.MAX_VALUE).

    private static void testUnsignedAccessor(Path seed) throws Exception {
        SizeStats st =
            Snapdir.manifestSize(Snapdir.manifest(seed.toString(), ManifestOptions.builder().build()));
        // COMPILE-PIN: physicalBytesUnsigned() is a String, per the sizeUnsigned() convention.
        String pu = st.physicalBytesUnsigned();
        check(pu != null && pu.equals(Long.toUnsignedString(st.physicalBytes())),
            "physicalBytesUnsigned() must equal Long.toUnsignedString(physicalBytes)");
        check(pu != null && pu.equals("29"),
            "physicalBytesUnsigned() must be \"29\" for the fixture, got " + pu);
    }

    // ================================================================ 8. degenerate: manifestSize of a File-less (dirs-only) manifest ⇒ all-zeros ================================================================
    //
    // clause: a manifest with only DIRECTORY entries (no File entries) has no
    // content — logicalBytes=0, physicalBytes=0, files=0, objects=0. Directory
    // entries carry checksum "-" and must NOT be counted as objects.

    private static void testDirsOnlyManifestZero() {
        // Two directory lines only (checksum "-" for dirs, per the manifest format).
        Manifest dirsOnly = Manifest.parse(
            "D 0755 - 0 ./onlydir/\n" +
            "D 0755 - 0 ./onlydir/nested/\n");
        boolean sawOnlyDirs = true;
        for (ManifestEntry e : dirsOnly.entries()) {
            if (e.type() != PathType.DIRECTORY) sawOnlyDirs = false;
        }
        check(sawOnlyDirs && !dirsOnly.entries().isEmpty(),
            "precondition: the degenerate manifest must parse to DIRECTORY-only entries");
        SizeStats st = Snapdir.manifestSize(dirsOnly);
        check(st != null && st.logicalBytes() == 0L && st.physicalBytes() == 0L
                && st.files() == 0L && st.objects() == 0L,
            "manifestSize of a dirs-only manifest must be all-zeros, got " + st);
    }

    // ================================================================ 9. HARDENING: cross-snapshot dedup — two snapshots, ONE store, shared object weighed ONCE ================================================================
    //
    // clause: push TWO distinct snapshots into ONE store. Snapshot A is the seed;
    // snapshot B shares A's "hello world\n" (12) object but adds a distinct object.
    // Per-id size(store,id) reproduces each snapshot's OWN manifestSize figure.
    // The whole-store size ADDS files and logicalBytes (nothing is deduped across
    // snapshots for the logical view) but STRICTLY dedups physicalBytes and objects
    // (the shared "hello world\n" object is stored once). All reference values are
    // COMPUTED from the local manifests / on-disk bytes — no hardcoded counts.

    private static void testCrossSnapshotDedup(Path seedA, Path root) throws Exception {
        // Second tree B: one file that shares A's 12-byte object, one brand-new object.
        Path seedB = root.resolve("seedB");
        Files.createDirectories(seedB);
        Files.write(seedB.resolve("shared.txt"), "hello world\n".getBytes());        // dup of A's x.txt
        Files.write(seedB.resolve("fresh.txt"), "brand new content!\n".getBytes());  // distinct object

        Path storeDir = root.resolve("store-xsnap");
        String store = storeUri(storeDir);

        // Push both snapshots into ONE store (match the real push(String,String,PushOptions) signature).
        String idA = Snapdir.push(seedA.toString(), store, null).get();
        String idB = Snapdir.push(seedB.toString(), store, null).get();
        check(idA != null && idA.matches("^[0-9a-f]{64}$"), "push(seedA) must return a 64-hex id, got " + idA);
        check(idB != null && idB.matches("^[0-9a-f]{64}$"), "push(seedB) must return a 64-hex id, got " + idB);
        check(!idA.equals(idB), "the two distinct snapshots must have distinct ids: " + idA + " vs " + idB);

        // Reference figures computed from the LOCAL manifests (no iteration, no hardcoding).
        SizeStats a = Snapdir.manifestSize(Snapdir.manifest(seedA.toString(), ManifestOptions.builder().build()));
        SizeStats b = Snapdir.manifestSize(Snapdir.manifest(seedB.toString(), ManifestOptions.builder().build()));
        check(a != null && b != null, "reference manifestSize figures must be non-null");

        // Per-id selection reproduces each snapshot's OWN figure.
        check(Snapdir.size(store, idA).get().equals(a), "size(store, idA) must equal manifestSize(A): got " + Snapdir.size(store, idA).get() + " vs " + a);
        check(Snapdir.size(store, idB).get().equals(b), "size(store, idB) must equal manifestSize(B): got " + Snapdir.size(store, idB).get() + " vs " + b);

        SizeStats whole = Snapdir.size(store, null).get();
        check(whole != null, "whole-store size must be non-null");
        if (whole != null) {
            // files + logicalBytes ADD across snapshots (no cross-snapshot logical dedup).
            check(whole.files() == a.files() + b.files(),
                "whole.files must be a.files+b.files=" + (a.files() + b.files()) + ", got " + whole.files());
            check(whole.logicalBytes() == a.logicalBytes() + b.logicalBytes(),
                "whole.logicalBytes must be a+b=" + (a.logicalBytes() + b.logicalBytes()) + ", got " + whole.logicalBytes());
            // physicalBytes + objects STRICTLY shrink: the shared "hello world\n" object is stored ONCE.
            check(Long.compareUnsigned(whole.physicalBytes(), a.physicalBytes() + b.physicalBytes()) < 0,
                "whole.physicalBytes must be STRICTLY less than a+b=" + (a.physicalBytes() + b.physicalBytes()) + " (cross-snapshot dedup), got " + whole.physicalBytes());
            check(Long.compareUnsigned(whole.objects(), a.objects() + b.objects()) < 0,
                "whole.objects must be STRICTLY less than a+b=" + (a.objects() + b.objects()) + " (shared object counted once), got " + whole.objects());
            // ACCEPTANCE: whole physicalBytes == real on-disk .objects/ byte sum (uncompressed).
            long onDisk = objectsBytesOnDisk(storeDir);
            check(onDisk >= 0L, "the cross-snapshot store must have a .objects/ directory to weigh");
            check(whole.physicalBytes() == onDisk,
                "whole.physicalBytes must EQUAL the real .objects/ byte total: " + whole.physicalBytes() + " vs " + onDisk);
        }
        // Re-run determinism: two whole-store sizes are equal.
        check(Snapdir.size(store, null).get().equals(Snapdir.size(store, null).get()),
            "re-running whole-store size must be deterministic (equal SizeStats)");
    }

    // ---------------------------------------------------------------- main ----------------------------------------------------------------

    public static void main(String[] args) throws Exception {
        // Hermetic + offline: a private temp root; the runner exports
        // SNAPDIR_CACHE_DIR / SNAPDIR_CATALOG_DB_PATH into the env (mirrors the
        // api spec's hermetic anchor). file:// stores only, no network.
        Path root = Files.createTempDirectory("snapdir-java-size-");
        Path seed = buildSeed(root.resolve("seed"));
        Path storeDir = root.resolve("store");

        try {
            testManifestSizeFigure(seed);            // 1
            testSizeOfPushedSnapshot(seed, storeDir);// 2 (also creates the store)
            testWholeStoreSize(storeDir);            // 3
            testPhysicalBytesMatchDisk(storeDir);    // 4 (ACCEPTANCE)
            testStrictInequalities(seed);            // 5
            testStoreErrorContract(root);            // 6 (a)/(b)/(c)
            testUnsignedAccessor(seed);              // 7
            testDirsOnlyManifestZero();              // 8
            testCrossSnapshotDedup(seed, root);      // 9 (HARDENING: cross-snapshot dedup)
        } finally {
            // best-effort cleanup of the private temp root.
            try (Stream<Path> s = Files.walk(root)) {
                s.sorted((a, b) -> b.getNameCount() - a.getNameCount())
                 .forEach(p -> { try { Files.deleteIfExists(p); } catch (IOException ignored) {} });
            } catch (IOException ignored) {}
        }

        System.err.println("checks: " + checks + ", failures: " + failures);
        System.exit(failures == 0 ? 0 : 1);
    }
}
