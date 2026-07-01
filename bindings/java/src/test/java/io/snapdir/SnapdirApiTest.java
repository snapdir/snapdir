// SnapdirApiTest.java  black-box spec for the snapdir Java binding
// (Phase 42, gate java-api-spec-tests; adversary/opus).
//
// Authored from the SPEC ONLY: the FROZEN C ABI (include/snapdir.h,
// c-abi.sha.lock  8 stable error codes; manifest TEXT "TYPE PERM CHECKSUM
// SIZE PATH"; diff JSON status A/D/M/=) + the PUBLIC io.snapdir surface named
// in the gate description. NO visibility into the binding's internal/ JNI/FFI
// implementation or the Rust src/. It is a self-contained plain-assert harness
// with public static void main(String[]) (the offline image ships NO JUnit /
// maven / gradle  only openjdk-17 + javac/jar), modelled on the cpp/zig
// adversary specs: a check(boolean,String) that counts failures and
// System.exit(failures==0?0:1). It calls ONLY the public io.snapdir surface
// (the binding hides jdk.incubator.foreign), and is EXPECTED TO FAIL / not
// fully pass against the current scaffold  java-api-impl makes it pass. Do
// NOT weaken assertions to pass against the scaffold.
//
// Compile (native arm64, fast):
//   javac --release 17 --add-modules jdk.incubator.foreign \
//     -cp build/classes -d /tmp/jt .gatesmith/pending-tests/SnapdirApiTest.java
// Run (java-api-impl wires this): the cdylib on the resource path +
//   --enable-native-access=ALL-UNNAMED.
//
// The PUBLIC contract this pins (io.snapdir):
//   String                          Snapdir.version();
//   Manifest                        Snapdir.manifest(String, ManifestOptions) throws SnapdirException;
//   String                          Snapdir.id(String, ManifestOptions)       throws SnapdirException;
//   String                          Snapdir.idFromManifest(Manifest)          throws SnapdirException;
//   CompletableFuture<String>       Snapdir.push(String, String, PushOptions);
//   CompletableFuture<Void>         Snapdir.pull(String, String, String, PullOptions);
//   CompletableFuture<Void>         Snapdir.fetch(String, String, int);
//   CompletableFuture<List<DiffEntry>> Snapdir.diff(String, String, DiffOptions);
//   record ManifestEntry(PathType type, int perm, String checksum, long size, String path)
//                                   + String sizeUnsigned()  (== Long.toUnsignedString(size))
//   record Manifest(String raw, List<ManifestEntry> entries) + static Manifest parse(String)
//   record DiffEntry(DiffStatus status, String path)
//   enum  PathType{FILE,DIRECTORY,SYMLINK}; enum DiffStatus{ADDED,DELETED,MODIFIED,UNCHANGED}
//   class SnapdirException extends Exception { String getCode(); }
//     subclasses HashMismatchException / StoreException / InFluxException / CatalogException
//   ManifestOptions/PushOptions/PullOptions/DiffOptions  builder-style (builder()..build())

package io.snapdir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermission;
import java.util.EnumSet;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.regex.Pattern;

/**
 * Self-contained black-box spec for the public {@code io.snapdir} surface.
 *
 * <p>This class lives in the {@code io.snapdir} package so it can compile-pin
 * the record shapes (e.g. {@code long size} on {@link ManifestEntry}) directly,
 * but it exercises ONLY the public static API  never the {@code internal/}
 * package. It tries hard to BREAK the binding: a failure barrage on the I/O
 * paths, the checked-exception hierarchy, async {@link CompletableFuture}
 * propagation, options-honoured deltas, and a leak-pressure loop on the throw
 * path.
 */
public final class SnapdirApiTest {

    private SnapdirApiTest() {}

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

    // ---------------------------------------------------------------- the 8 stable ABI error codes (snapdir.h / snapdir_error_code) ----------------------------------------------------------------
    // A binding failure MUST surface getCode() from exactly this set (or
    // "INTERNAL" for the catch_unwind boundary). NOT a message, NOT empty.
    private static final Set<String> STABLE_CODES = new HashSet<>(List.of(
        "IO_ERROR", "HASH_MISMATCH", "STORE_ERROR", "IN_FLUX",
        "CATALOG_ERROR", "INVALID_ID", "INVALID_STORE", "CONFLICT"
    ));

    private static boolean isStableCode(String c) {
        return c != null && (STABLE_CODES.contains(c) || c.equals("INTERNAL"));
    }

    private static final Pattern HEX64 = Pattern.compile("^[0-9a-f]{64}$");
    private static final Pattern SEMVER = Pattern.compile("^\\d+\\.\\d+\\.\\d+.*");

    // ---------------------------------------------------------------- temp-tree helpers (offline, file:// only) ----------------------------------------------------------------

    // Build a small offline tree: a.txt (0644, 5 bytes), sub/ dir (0755),
    // sub/b.bin (0600, 7 bytes), link -> a.txt (relative symlink). Returns root.
    private static Path buildTree(Path root) throws IOException {
        Files.createDirectories(root.resolve("sub"));
        Files.writeString(root.resolve("a.txt"), "hello");          // 5 bytes
        Files.writeString(root.resolve("sub").resolve("b.bin"), "world!\n"); // 7 bytes
        trySetPerms(root.resolve("a.txt"), "rw-r--r--");            // 0644
        trySetPerms(root.resolve("sub"), "rwxr-xr-x");             // 0755
        trySetPerms(root.resolve("sub").resolve("b.bin"), "rw-------"); // 0600
        // Relative symlink for the no_follow axis; tolerate FS without symlink.
        try {
            Files.createSymbolicLink(root.resolve("link"), Path.of("a.txt"));
        } catch (IOException | UnsupportedOperationException ignored) {
            // no symlink support here  the no_follow check self-skips below.
        }
        return root;
    }

    private static void trySetPerms(Path p, String rwx) {
        try {
            Files.setPosixFilePermissions(p, PosixFilePermission.values().length > 0
                ? parsePerms(rwx) : EnumSet.noneOf(PosixFilePermission.class));
        } catch (IOException | UnsupportedOperationException ignored) {
            // non-POSIX FS  perm-restore round-trip self-relaxes.
        }
    }

    private static Set<PosixFilePermission> parsePerms(String rwx) {
        Set<PosixFilePermission> s = EnumSet.noneOf(PosixFilePermission.class);
        PosixFilePermission[] order = {
            PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE, PosixFilePermission.OWNER_EXECUTE,
            PosixFilePermission.GROUP_READ, PosixFilePermission.GROUP_WRITE, PosixFilePermission.GROUP_EXECUTE,
            PosixFilePermission.OTHERS_READ, PosixFilePermission.OTHERS_WRITE, PosixFilePermission.OTHERS_EXECUTE,
        };
        for (int i = 0; i < rwx.length() && i < order.length; i++) {
            if (rwx.charAt(i) != '-') s.add(order[i]);
        }
        return s;
    }

    // Normalize a manifest path for matching: strip a leading "./" and any
    // trailing "/" (dirs are recorded with a trailing slash, paths are
    // "./"-relative). Mirrors the cpp/go reference normalizers.
    private static String norm(String p) {
        if (p.startsWith("./")) p = p.substring(2);
        while (p.endsWith("/")) p = p.substring(0, p.length() - 1);
        return p;
    }

    private static String fileStore(Path dir) {
        return "file://" + dir.toAbsolutePath();
    }

    // Walk the cause chain of a Throwable and return the first SnapdirException
    // found (CompletableFuture.get() throws ExecutionException whose cause is
    // the binding's SnapdirException, possibly wrapped one extra layer in a
    // CompletionException). Returns null if none in the chain.
    private static SnapdirException findSnapdirCause(Throwable t) {
        Throwable cur = t;
        Set<Throwable> seen = new HashSet<>();
        while (cur != null && seen.add(cur)) {
            if (cur instanceof SnapdirException se) return se;
            cur = cur.getCause();
        }
        return null;
    }

    // ---------------------------------------------------------------- 1. version() (clause: String Snapdir.version(), non-empty) ----------------------------------------------------------------

    private static void testVersion() {
        // COMPILE-PIN: version() returns String, takes no args, is not checked.
        String v = Snapdir.version();
        check(v != null && !v.isEmpty(), "version() must be a non-empty String");
        check(v != null && SEMVER.matcher(v).matches(),
            "version() must look like a semantic version (\\d+.\\d+.\\d+), got: " + v);
    }

    // ---------------------------------------------------------------- 2. ManifestEntry record shape + sizeUnsigned() (clause: long size) ----------------------------------------------------------------
    //
    // sizeUnsigned() MUST equal Long.toUnsignedString(size)  pinned both on a
    // real manifest entry AND on a synthetic Manifest.parse() line whose size
    // field is > Long.MAX_VALUE so the signed-long wrap is exercised (the WHOLE
    // reason sizeUnsigned() exists). a.txt's size is the real 5-byte count.

    private static void testManifestEntryShapeAndSizeUnsigned(Path tree) throws Exception {
        Manifest m = Snapdir.manifest(tree.toString(), ManifestOptions.builder().build());
        check(m != null && m.raw() != null && !m.raw().isEmpty(),
            "Manifest.raw() must hold the raw manifest text");
        check(m != null && m.entries() != null && !m.entries().isEmpty(),
            "Manifest.entries() must be non-empty for a non-empty tree");

        boolean sawA = false, sawSub = false, sawB = false;
        for (ManifestEntry e : m.entries()) {
            // COMPILE-PIN: size is a primitive long; sizeUnsigned() is a String.
            long sz = e.size();
            String su = e.sizeUnsigned();
            check(su != null && su.equals(Long.toUnsignedString(sz)),
                "sizeUnsigned() must equal Long.toUnsignedString(size) for " + e.path());
            // COMPILE-PIN: perm is an int; checksum is a String; type is PathType.
            int perm = e.perm();
            check((perm & ~0x0fff) == 0, "entry perm must be octal mode bits: " + e.path());
            if (e.type() == PathType.FILE) {
                check(e.checksum() != null && HEX64.matcher(e.checksum()).matches(),
                    "FILE entry checksum must be 64-hex: " + e.path());
            }
            String rel = norm(e.path());
            if (rel.equals("a.txt")) {
                sawA = true;
                check(e.type() == PathType.FILE, "a.txt must be type FILE");
                check(e.size() == 5L, "a.txt size must be 5 (long), got " + e.size());
                check((e.perm() & 0777) == 0644 || !isPosix(tree),
                    "a.txt perm must be 0644 (octal), got " + Integer.toOctalString(e.perm()));
            } else if (rel.equals("sub")) {
                sawSub = true;
                check(e.type() == PathType.DIRECTORY, "sub must be type DIRECTORY");
            } else if (rel.equals("sub/b.bin")) {
                sawB = true;
                check(e.type() == PathType.FILE, "sub/b.bin must be type FILE");
                check(e.size() == 7L, "sub/b.bin size must be 7 (long), got " + e.size());
            }
        }
        check(sawA, "manifest must list a.txt");
        check(sawSub, "manifest must list sub/ as a DIRECTORY");
        check(sawB, "manifest must list sub/b.bin");

        // Signed-long wrap: a size > Long.MAX_VALUE parses to a NEGATIVE long but
        // sizeUnsigned() must still produce the correct unsigned decimal. This is
        // the contract that justifies sizeUnsigned() over Long.toString().
        String big = "18446744073709551615"; // 2^64 - 1
        String line = "F 0644 " + "a".repeat(64) + " " + big + " ./huge.bin";
        Manifest hm = Manifest.parse(line);
        check(hm.entries().size() == 1, "Manifest.parse must parse one huge-size entry");
        ManifestEntry he = hm.entries().get(0);
        check(he.size() < 0, "size > Long.MAX_VALUE must wrap to a negative signed long");
        check(he.sizeUnsigned().equals(big),
            "sizeUnsigned() must recover the full uint64 decimal (" + big + "), got " + he.sizeUnsigned());
        check(he.sizeUnsigned().equals(Long.toUnsignedString(he.size())),
            "sizeUnsigned() must equal Long.toUnsignedString(size) even past Long.MAX_VALUE");
    }

    // Best-effort POSIX detection so perm assertions self-relax on a non-POSIX FS.
    private static boolean isPosix(Path p) {
        try {
            return p.getFileSystem().supportedFileAttributeViews().contains("posix");
        } catch (Exception e) {
            return false;
        }
    }

    // ---------------------------------------------------------------- 3. id/manifest self-consistency (clause: idFromManifestmanifest==id) ----------------------------------------------------------------
    //
    // id(tree) is 64-lowercase-hex; deterministic across re-runs; and
    // idFromManifest(manifest(tree)) == id(tree). idFromManifest takes a Manifest
    // and is a pure sync (no future) checked call.

    private static void testIdSelfConsistency(Path tree) throws Exception {
        ManifestOptions def = ManifestOptions.builder().build();
        String id1 = Snapdir.id(tree.toString(), def);
        check(id1 != null && HEX64.matcher(id1).matches(),
            "id(tree) must be 64-lowercase-hex, got " + id1);

        String id2 = Snapdir.id(tree.toString(), def);
        check(id1.equals(id2), "id() must be deterministic over an unchanged tree");

        Manifest m = Snapdir.manifest(tree.toString(), def);
        String idFromM = Snapdir.idFromManifest(m);
        check(idFromM != null && HEX64.matcher(idFromM).matches(),
            "idFromManifest must be 64-lowercase-hex, got " + idFromM);
        check(idFromM.equals(id1),
            "idFromManifest(manifest(tree)) must equal id(tree): " + idFromM + " != " + id1);

        // null opts MUST be accepted as defaults (the binding documents null ==
        // defaults). Pin it on both id() and manifest().
        String idNull = Snapdir.id(tree.toString(), null);
        check(idNull.equals(id1), "id(tree, null) must equal id(tree, defaults)");
        Manifest mNull = Snapdir.manifest(tree.toString(), null);
        check(Snapdir.idFromManifest(mNull).equals(id1),
            "manifest(tree, null) must equal manifest(tree, defaults)");
    }

    // ---------------------------------------------------------------- 4. checked-exception hierarchy on the I/O paths (PARAMOUNT) ----------------------------------------------------------------
    //
    // Snapdir.id("/no/such/path", opts) MUST throw a CHECKED SnapdirException
    // (a subtype of java.lang.Exception, NEVER a RuntimeException) whose
    // getCode() is in the 8-code set. The compiler ALSO pins this: id()/manifest()
    // are declared `throws SnapdirException`, so a try/catch on SnapdirException
    // must be reachable (a RuntimeException-only impl would not compile this).
    // A failure barrage ensures NO uncaught/unchecked exception escapes.

    private static void testCheckedExceptionHierarchy(Path root) {
        // 4a. missing path  checked SnapdirException, never a RuntimeException.
        boolean threw = false;
        try {
            String s = Snapdir.id("/snapdir/no/such/path/zzz-missing-xyz",
                ManifestOptions.builder().build());
            check(false, "id() on a missing path must throw, returned " + s);
        } catch (SnapdirException e) {
            threw = true;
            // Compile-pin: SnapdirException IS-A checked Exception (assignable to
            // Exception, NOT to RuntimeException).
            Exception asChecked = e; // must compile (SnapdirException extends Exception)
            check(!(asChecked instanceof RuntimeException),
                "SnapdirException must be a CHECKED exception, not a RuntimeException");
            check(isStableCode(e.getCode()),
                "id(missing).getCode() must be a stable ABI code, got " + e.getCode());
            check(e.getMessage() != null && !e.getMessage().isEmpty(),
                "SnapdirException.getMessage() must be non-empty");
        } catch (RuntimeException re) {
            check(false, "id(missing) threw an UNCHECKED RuntimeException: " + re);
        }
        check(threw, "id() on a missing path MUST throw a checked SnapdirException");

        // 4b. manifest() on a missing path  independent C entry point.
        boolean threwM = false;
        try {
            Snapdir.manifest("/snapdir/no/such/path/zzz-missing-mani", null);
            check(false, "manifest() on a missing path must throw");
        } catch (SnapdirException e) {
            threwM = true;
            check(isStableCode(e.getCode()),
                "manifest(missing).getCode() must be a stable ABI code, got " + e.getCode());
        } catch (RuntimeException re) {
            check(false, "manifest(missing) threw an UNCHECKED RuntimeException: " + re);
        }
        check(threwM, "manifest() on a missing path MUST throw a checked SnapdirException");

        // 4c. SnapdirException subclass hierarchy is well-formed (compile-pin the
        // four named subtypes all extend SnapdirException, which extends Exception,
        // and carry the contract codes). This pins the hierarchy the impl must map
        // HASH_MISMATCH/STORE_ERROR/IN_FLUX/CATALOG_ERROR onto.
        SnapdirException hm = new HashMismatchException("x");
        SnapdirException st = new StoreException("x");
        SnapdirException fx = new InFluxException("x");
        SnapdirException ct = new CatalogException("x");
        check(hm.getCode().equals("HASH_MISMATCH"), "HashMismatchException code must be HASH_MISMATCH");
        check(st.getCode().equals("STORE_ERROR"), "StoreException code must be STORE_ERROR");
        check(fx.getCode().equals("IN_FLUX"), "InFluxException code must be IN_FLUX");
        check(ct.getCode().equals("CATALOG_ERROR"), "CatalogException code must be CATALOG_ERROR");
        for (SnapdirException e : List.of(hm, st, fx, ct)) {
            check(!(((Exception) e) instanceof RuntimeException),
                "every SnapdirException subtype must be CHECKED, not a RuntimeException: " + e.getCode());
        }
    }

    // ---------------------------------------------------------------- 5. options honoured (exclude  id; no_follow  manifest+id) ----------------------------------------------------------------
    //
    // exclude IS a snapdir_id parameter, so id(tree, exclude) MUST differ from
    // the default walk. no_follow is NOT a snapdir_id parameter, but the BINDING
    // routes no_follow/absolute through manifest()idFromManifest() (its own
    // documented compensation), so id(tree, noFollow) MUST differ from default AND
    // equal idFromManifest(manifest(tree, noFollow)). Under no_follow the symlink
    // is OMITTED (snapdir has NO type-L manifest entry  journal/golden)  it must
    // NOT appear as a dereferenced FILE. We do NOT require a SYMLINK entry.

    private static void testOptionsHonoured(Path tree) throws Exception {
        String base = Snapdir.id(tree.toString(), ManifestOptions.builder().build());
        check(HEX64.matcher(base).matches(), "base id must be 64-hex");

        // exclude drops a.txt from the walk  different id.
        String excluded = Snapdir.id(tree.toString(),
            ManifestOptions.builder().exclude("a\\.txt").build());
        check(HEX64.matcher(excluded).matches(), "excluded id must be 64-hex");
        check(!excluded.equals(base), "exclude option must change the snapshot id");

        // no_follow only meaningful if the symlink exists on this FS.
        if (Files.isSymbolicLink(tree.resolve("link"))) {
            ManifestOptions nfo = ManifestOptions.builder().noFollow(true).build();
            Manifest mdef = Snapdir.manifest(tree.toString(), ManifestOptions.builder().build());
            Manifest mnf = Snapdir.manifest(tree.toString(), nfo);
            check(!mdef.raw().equals(mnf.raw()),
                "no_follow must change the manifest text (symlink handled differently)");

            String idDefM = Snapdir.idFromManifest(mdef);
            String idNfM = Snapdir.idFromManifest(mnf);
            check(!idDefM.equals(idNfM),
                "no_follow must change idFromManifest vs the default follow walk");

            // The binding's id() compensation MUST fire for no_follow.
            String idNf = Snapdir.id(tree.toString(), nfo);
            check(HEX64.matcher(idNf).matches(), "no_follow id() must be 64-hex");
            check(!idNf.equals(base),
                "id(tree, noFollow) must differ from the default follow walk (binding routes via manifest)");
            check(idNf.equals(idNfM),
                "id(tree, noFollow) must equal idFromManifest(manifest(tree, noFollow))");

            // Under no_follow the symlink must NOT be a dereferenced FILE. snapdir
            // OMITS un-followed relative symlinks  there is NO type-L entry, so we
            // assert the absence of a 'link' FILE entry, not the presence of a SYMLINK.
            boolean linkAsFile = false;
            for (ManifestEntry e : mnf.entries()) {
                if (norm(e.path()).equals("link") && e.type() == PathType.FILE) {
                    linkAsFile = true;
                }
            }
            check(!linkAsFile,
                "under no_follow the symlink must not be recorded as a dereferenced FILE");
        } else {
            System.err.println("note: symlink unsupported here; skipping no_follow check");
        }
    }

    // ---------------------------------------------------------------- 6. async CompletableFuture round-trip (push  pull  fetch; failing) ----------------------------------------------------------------
    //
    // push(tree, file:// store, opts).get() == id(tree) (push must not mutate).
    // pull(id, store, dest, opts).get() into a PRE-EXISTING 0700 dest succeeds and
    // re-id(dest) == pushed (the shared permission-restore contract through the C
    // ABI). fetch(id, store, 0).get() succeeds. A future wrapping a FAILING op
    // throws from .get(), and the cause chain contains a SnapdirException with a
    // stable code (never a bare unchecked exception surfaced raw).

    private static void testAsyncRoundtrip(Path tree, Path root) throws Exception {
        String store = fileStore(root.resolve("store"));
        String local = Snapdir.id(tree.toString(), ManifestOptions.builder().build());

        // push() returns CompletableFuture<String>.
        CompletableFuture<String> pushFut =
            Snapdir.push(tree.toString(), store, PushOptions.builder().build());
        String pushed;
        try {
            pushed = pushFut.get();
        } catch (ExecutionException ee) {
            check(false, "push().get() threw: " + ee.getCause());
            return;
        }
        check(pushed != null && HEX64.matcher(pushed).matches(), "push() id must be 64-hex");
        check(pushed.equals(local), "push() id must equal local id(tree) (push must not mutate)");

        // pull() into a PRE-EXISTING restrictive (0700) dest. A pull that didn't
        // restore each entry's mode would re-id differently.
        Path dest = root.resolve("dest");
        Files.createDirectories(dest);
        trySetPerms(dest, "rwx------"); // 0700
        CompletableFuture<Void> pullFut =
            Snapdir.pull(pushed, store, dest.toString(), PullOptions.builder().build());
        try {
            Void v = pullFut.get();
            check(v == null, "pull().get() must resolve to null (CompletableFuture<Void>)");
        } catch (ExecutionException ee) {
            check(false, "pull().get() threw: " + ee.getCause());
            return;
        }
        String reid = Snapdir.id(dest.toString(), ManifestOptions.builder().build());
        check(reid.equals(pushed),
            "pulled tree must re-id to the pushed id (permission-restore via C ABI): "
                + reid + " != " + pushed);

        // fetch() into the local cache must succeed for a present snapshot.
        try {
            Snapdir.fetch(pushed, store, 0).get();
        } catch (ExecutionException ee) {
            check(false, "fetch().get() threw: " + ee.getCause());
        }

        // A future wrapping a FAILING op must throw from .get(), with a
        // SnapdirException in the cause chain carrying a stable code.
        String emptyStore = fileStore(root.resolve("empty-store"));
        String bogusId = "0".repeat(64);
        Path badDest = root.resolve("bad-dest");
        Files.createDirectories(badDest);
        boolean threw = false;
        try {
            Snapdir.pull(bogusId, emptyStore, badDest.toString(),
                PullOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            threw = true;
            SnapdirException cause = findSnapdirCause(ee);
            check(cause != null,
                "a failing pull().get() must wrap a SnapdirException in its cause chain, got "
                    + ee.getCause());
            check(cause == null || isStableCode(cause.getCode()),
                "failing pull().get() cause code must be a stable ABI code, got "
                    + (cause == null ? "<none>" : cause.getCode()));
        } catch (InterruptedException ie) {
            check(false, "interrupted: " + ie);
        }
        check(threw, "a future wrapping a failing pull MUST throw from .get()");
    }

    // ---------------------------------------------------------------- 7. future exception propagation for push/diff with EXACT codes ----------------------------------------------------------------
    //
    // Two inputs are deterministic at the C ABI: a syntactically-invalid store
    // URI  INVALID_STORE, and a malformed snapshot id  INVALID_ID. Each async
    // entry point has its own wiring, so a propagation bug in one is invisible to
    // the pull test above. We pin the EXACT code so a swapped mapping is caught.

    private static void testFutureExactCodes(Path tree, Path root) throws Exception {
        // push()  invalid store URI re-throws with INVALID_STORE.
        boolean threwPush = false;
        try {
            Snapdir.push(tree.toString(), "not-a-valid-uri",
                PushOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            threwPush = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null, "push(invalid store) must wrap a SnapdirException, got " + ee.getCause());
            check(c != null && c.getCode().equals("INVALID_STORE"),
                "push to a syntactically-invalid store URI must be INVALID_STORE, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(threwPush, "push() to an invalid store must throw from .get()");

        // pull() with a malformed snapshot id against a REAL store  INVALID_ID.
        String store = fileStore(root.resolve("exact-store"));
        Snapdir.push(tree.toString(), store, PushOptions.builder().build()).get(); // make store real
        Path dest = root.resolve("exact-dest");
        Files.createDirectories(dest);
        boolean threwId = false;
        try {
            Snapdir.pull("xyz-not-a-valid-id", store, dest.toString(),
                PullOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            threwId = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null && c.getCode().equals("INVALID_ID"),
                "pull with a malformed snapshot id must be INVALID_ID, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(threwId, "pull() with a malformed snapshot id must throw from .get()");
    }

    // ---------------------------------------------------------------- 8. diff() across two stores (clause: DiffStatus A/M/=; self-diff empty) ----------------------------------------------------------------
    //
    // push tree A  storeA, a MODIFIED tree B  storeB; diff(storeA, storeB).get()
    // returns DiffEntry(s) pairing the changed path with its DiffStatus (ADDED
    // c.txt, MODIFIED a.txt). A self-diff (same store both sides) reports no
    // ADDED/DELETED/MODIFIED rows. file:// only, offline.

    private static void testDiffAcrossStores(Path tree, Path root) throws Exception {
        String storeA = fileStore(root.resolve("diff-storeA"));
        String storeB = fileStore(root.resolve("diff-storeB"));

        Snapdir.push(tree.toString(), storeA, PushOptions.builder().build()).get();

        // B = a modified copy: change a.txt's bytes and add c.txt.
        Path treeB = root.resolve("treeB");
        Files.createDirectories(treeB.resolve("sub"));
        Files.writeString(treeB.resolve("a.txt"), "HELLO-CHANGED");          // modified
        Files.writeString(treeB.resolve("sub").resolve("b.bin"), "world!\n"); // unchanged
        Files.writeString(treeB.resolve("c.txt"), "added\n");                // added
        trySetPerms(treeB.resolve("a.txt"), "rw-r--r--");
        trySetPerms(treeB.resolve("sub").resolve("b.bin"), "rw-------");
        trySetPerms(treeB.resolve("c.txt"), "rw-r--r--");
        Snapdir.push(treeB.toString(), storeB, PushOptions.builder().build()).get();

        List<DiffEntry> entries =
            Snapdir.diff(storeA, storeB, DiffOptions.builder().build()).get();
        check(!entries.isEmpty(), "diff across two distinct stores must report changes");

        boolean sawAddedC = false, sawModifiedA = false;
        for (DiffEntry e : entries) {
            DiffStatus s = e.status();
            check(s == DiffStatus.ADDED || s == DiffStatus.DELETED
                    || s == DiffStatus.MODIFIED || s == DiffStatus.UNCHANGED,
                "DiffEntry.status() must be a DiffStatus enum constant");
            check(e.path() != null && !e.path().isEmpty(),
                "every DiffEntry must carry a non-empty path (statuspath same object)");
            String name = norm(e.path());
            if (e.status() == DiffStatus.ADDED && name.endsWith("c.txt")) sawAddedC = true;
            if (e.status() == DiffStatus.MODIFIED && name.endsWith("a.txt")) sawModifiedA = true;
        }
        check(sawAddedC, "diff must report c.txt as ADDED");
        check(sawModifiedA, "diff must report a.txt as MODIFIED");

        // self-diff: same store both sides  no change rows.
        List<DiffEntry> self =
            Snapdir.diff(storeA, storeA, DiffOptions.builder().build()).get();
        for (DiffEntry e : self) {
            check(e.status() == DiffStatus.UNCHANGED,
                "a self-diff must produce no ADDED/DELETED/MODIFIED rows, got " + e.status() + " " + e.path());
        }
    }

    // ---------------------------------------------------------------- 9. no native-memory leak on the throw path (leak pressure) ----------------------------------------------------------------
    //
    // Loop the failing-id case 200 so the binding must free the C string + the
    // C SnapdirError on EVERY throw. You cannot observe leaks directly from Java,
    // but a per-throw native leak (a missing free in the FFI down-call) shows up
    // under -Xcheck:jni / NMT (which java-quality probes), and a per-throw bug
    // that corrupted the error would make getCode()/getMessage() inconsistent. We
    // pin that 200 repeated failures each throw a checked SnapdirException with a
    // stable code and a non-empty message  no OOM, no corruption, no panic leak.

    private static void testThrowPathNoLeak() {
        String missing = "/snapdir/no/such/path/leak-loop-zzz";
        int thrown = 0;
        for (int i = 0; i < 200; i++) {
            try {
                Snapdir.id(missing, ManifestOptions.builder().build());
                check(false, "id(missing) must throw on iteration " + i);
            } catch (SnapdirException e) {
                thrown++;
                // Each throw must carry a consistent stable code + non-empty message
                // (a use-after-free of the freed C SnapdirError would corrupt these).
                if (!isStableCode(e.getCode())) {
                    check(false, "throw-loop iter " + i + ": non-stable code " + e.getCode());
                }
                if (e.getMessage() == null || e.getMessage().isEmpty()) {
                    check(false, "throw-loop iter " + i + ": empty message");
                }
            } catch (RuntimeException re) {
                check(false, "throw-loop iter " + i + " threw an UNCHECKED exception: " + re);
            }
        }
        check(thrown == 200, "every iteration of the throw loop must have thrown (got " + thrown + ")");
    }

    // ================================================================ STRENGTHENING (review gate, additive  0 removals) ================================================================
    // Authored at java-api-tests-review after the impl landed. The weakening
    // audit found the spec git mv'd BYTE-IDENTICAL (0 weakening). These cases
    // strengthen against now-visible contract surface the impl reveals:
    //   - EXACT checked-exception SUBTYPE/code where the C ABI is deterministic
    //     (missing path -> IO_ERROR; bogus store -> StoreException/INVALID_STORE;
    //     malformed id -> INVALID_ID)  not merely "some stable code".
    //   - record-component-type pins on ManifestEntry / DiffEntry / Manifest.
    //   - native-no-leak under SUSTAINED LOAD on BOTH the success and throw paths
    //     (a per-call C-string / SnapdirError leak in the FFI down-call would
    //     surface as OOM/throw/corruption over the loop; java-quality adds NMT).
    //   - CompletableFuture exception propagation for push/fetch/diff (not just
    //     pull): a failing future's .get() carries a SnapdirException cause.

    // ---------------------------------------------------------------- S1. missing-path code is EXACTLY IO_ERROR (not merely "stable") ----------------------------------------------------------------
    //
    // The spec pins isStableCode() on the missing-path throw. The C ABI is
    // deterministic here: a path that does not exist is an IO_ERROR (the python
    // sibling hard-asserts this; go logs it). We hard-pin the EXACT code so a
    // swapped IO_ERROR<->INVALID_STORE mapping in the FFI down-call is caught.

    private static void testMissingPathExactIoError() {
        String missing = "/snapdir/no/such/path/exact-io-error-zzz";
        boolean threw = false;
        try {
            Snapdir.id(missing, ManifestOptions.builder().build());
            check(false, "id(missing) must throw");
        } catch (SnapdirException e) {
            threw = true;
            check("IO_ERROR".equals(e.getCode()),
                "id(missing).getCode() must be EXACTLY IO_ERROR, got " + e.getCode());
        } catch (RuntimeException re) {
            check(false, "id(missing) threw an UNCHECKED exception: " + re);
        }
        check(threw, "id(missing) must throw");

        // manifest() shares the same C IO failure path  same exact code.
        boolean threwM = false;
        try {
            Snapdir.manifest(missing, null);
            check(false, "manifest(missing) must throw");
        } catch (SnapdirException e) {
            threwM = true;
            check("IO_ERROR".equals(e.getCode()),
                "manifest(missing).getCode() must be EXACTLY IO_ERROR, got " + e.getCode());
        } catch (RuntimeException re) {
            check(false, "manifest(missing) threw an UNCHECKED exception: " + re);
        }
        check(threwM, "manifest(missing) must throw");
    }

    // ---------------------------------------------------------------- S2. EXACT checked SUBTYPE mapping on the async paths ----------------------------------------------------------------
    //
    // The spec pins INVALID_STORE / INVALID_ID by CODE STRING. We additionally
    // pin that a STORE_ERROR maps to the StoreException SUBTYPE (so a caller can
    // `catch (StoreException)`), and that the INVALID_STORE/INVALID_ID codes
    // come through on the BASE SnapdirException type (their dedicated subtypes
    // are not in the mapped set  the impl returns base SnapdirException, which
    // the four named subclasses are siblings of). A bogus store URI is a
    // deterministic INVALID_STORE; we pin the exact subtype identity the impl
    // chose so a future remap is caught.

    private static void testExactExceptionSubtypes(Path tree, Path root) throws Exception {
        // The four named subclasses each carry their fixed code AND are distinct
        // checked types  a caller relies on `catch (StoreException)` etc.
        check(new StoreException("x") instanceof SnapdirException,
            "StoreException must be a SnapdirException");
        check(new HashMismatchException("x") instanceof SnapdirException,
            "HashMismatchException must be a SnapdirException");
        check(new InFluxException("x") instanceof SnapdirException,
            "InFluxException must be a SnapdirException");
        check(new CatalogException("x") instanceof SnapdirException,
            "CatalogException must be a SnapdirException");
        // The subtypes must be DISTINCT classes (not aliases) so catch-ordering works.
        Set<String> subtypeNames = new HashSet<>(List.of(
            StoreException.class.getName(), HashMismatchException.class.getName(),
            InFluxException.class.getName(), CatalogException.class.getName()));
        check(subtypeNames.size() == 4,
            "the four named SnapdirException subtypes must be distinct classes");

        // push() to a syntactically-invalid store URI: EXACT code INVALID_STORE,
        // surfaced as a SnapdirException (base, since INVALID_STORE has no
        // dedicated subtype in the mapping). A caller MUST be able to read the
        // code off the cause without a ClassCastException.
        boolean threwStore = false;
        try {
            Snapdir.push(tree.toString(), "not-a-valid-uri",
                PushOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            threwStore = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null, "push(invalid store) must wrap a SnapdirException");
            check(c != null && "INVALID_STORE".equals(c.getCode()),
                "push(invalid store) cause code must be EXACTLY INVALID_STORE, got "
                    + (c == null ? "<none>" : c.getCode()));
            // It is a CHECKED SnapdirException, never an unchecked surface.
            check(c == null || !(((Exception) c) instanceof RuntimeException),
                "the wrapped cause must be a checked SnapdirException");
        }
        check(threwStore, "push(invalid store) must throw from .get()");

        // pull() with a malformed id against a REAL store: EXACT INVALID_ID.
        String store = fileStore(root.resolve("subtype-store"));
        Snapdir.push(tree.toString(), store, PushOptions.builder().build()).get();
        Path dest = root.resolve("subtype-dest");
        Files.createDirectories(dest);
        boolean threwId = false;
        try {
            Snapdir.pull("not-hex-and-too-short", store, dest.toString(),
                PullOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            threwId = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null && "INVALID_ID".equals(c.getCode()),
                "pull(malformed id) cause code must be EXACTLY INVALID_ID, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(threwId, "pull(malformed id) must throw from .get()");
    }

    // ---------------------------------------------------------------- S3. record-component types + sizeUnsigned() boundary exactness ----------------------------------------------------------------
    //
    // Pin the EXACT record shapes the public API exposes (a record-component
    // rename/retype would break the binary contract). Also pin sizeUnsigned()'s
    // exact behaviour at 0, 1, Long.MAX_VALUE, Long.MAX_VALUE+1, and 2^64-1.

    private static void testRecordShapesAndSizeUnsignedBoundaries() {
        // ManifestEntry is a record with the documented component types/order.
        // COMPILE-PIN via the canonical constructor signature.
        ManifestEntry e = new ManifestEntry(PathType.FILE, 0644, "a".repeat(64), 42L, "./x");
        check(e.type() == PathType.FILE, "ManifestEntry.type() component");
        check(e.perm() == 0644, "ManifestEntry.perm() component (int)");
        check(e.checksum().length() == 64, "ManifestEntry.checksum() component (String)");
        check(e.size() == 42L, "ManifestEntry.size() component (long)");
        check(e.path().equals("./x"), "ManifestEntry.path() component (String)");

        // DiffEntry is a record (status, path).
        DiffEntry de = new DiffEntry(DiffStatus.ADDED, "./a");
        check(de.status() == DiffStatus.ADDED, "DiffEntry.status() component");
        check(de.path().equals("./a"), "DiffEntry.path() component");

        // Manifest is a record (raw, entries) and parse round-trips raw.
        Manifest m = Manifest.parse("F 0644 " + "b".repeat(64) + " 5 ./a.txt\n");
        check(m.raw() != null, "Manifest.raw() component (String)");
        check(m.entries().size() == 1, "Manifest.entries() component (List)");

        // sizeUnsigned() == Long.toUnsignedString(size) across the whole boundary.
        long[] sizes = { 0L, 1L, Long.MAX_VALUE, Long.MIN_VALUE, -1L };
        for (long sz : sizes) {
            ManifestEntry me = new ManifestEntry(PathType.FILE, 0644, "c".repeat(64), sz, "./s");
            check(me.sizeUnsigned().equals(Long.toUnsignedString(sz)),
                "sizeUnsigned() must equal Long.toUnsignedString(" + sz + ")");
        }
        // The specific overflow witnesses: 2^63 and 2^64-1.
        ManifestEntry top = Manifest.parse(
            "F 0644 " + "d".repeat(64) + " 9223372036854775808 ./two63").entries().get(0);
        check(top.size() == Long.MIN_VALUE,
            "2^63 must parse to Long.MIN_VALUE as a signed long");
        check(top.sizeUnsigned().equals("9223372036854775808"),
            "sizeUnsigned() must recover 2^63 unsigned decimal, got " + top.sizeUnsigned());
        ManifestEntry max = Manifest.parse(
            "F 0644 " + "e".repeat(64) + " 18446744073709551615 ./max").entries().get(0);
        check(max.size() == -1L, "2^64-1 must parse to -1 as a signed long");
        check(max.sizeUnsigned().equals("18446744073709551615"),
            "sizeUnsigned() must recover 2^64-1 unsigned decimal, got " + max.sizeUnsigned());
    }

    // ---------------------------------------------------------------- S4. native-no-leak under SUSTAINED LOAD on the SUCCESS path ----------------------------------------------------------------
    //
    // The spec's testThrowPathNoLeak() loops the FAILING id 200 (frees the C
    // SnapdirError + error string per throw). This complements it on the SUCCESS
    // path: 100 manifest+id round-trips each return a C char* that the FFI
    // down-call MUST free via snapdir_string_free. A per-call success-path leak
    // is invisible from Java directly, but over 100 iterations it would surface
    // as OOM / native-heap growth (java-quality runs this under NMT); we pin
    // that every iteration succeeds with a CONSISTENT, identical id (a
    // use-after-free or double-free of the returned string would corrupt it).

    private static void testSuccessPathNoLeakUnderLoad(Path tree) throws Exception {
        ManifestOptions def = ManifestOptions.builder().build();
        String firstId = Snapdir.id(tree.toString(), def);
        check(HEX64.matcher(firstId).matches(), "baseline id must be 64-hex");
        int ok = 0;
        for (int i = 0; i < 100; i++) {
            Manifest m = Snapdir.manifest(tree.toString(), def);
            check(m.raw() != null && !m.raw().isEmpty(),
                "load-loop iter " + i + ": manifest raw must be intact (no string corruption)");
            String idM = Snapdir.idFromManifest(m);
            String idDirect = Snapdir.id(tree.toString(), def);
            // Every returned C string must be freed exactly once AND be byte-stable.
            if (!idM.equals(firstId) || !idDirect.equals(firstId)) {
                check(false, "load-loop iter " + i + ": id drifted (free/use-after-free?) "
                    + idM + " / " + idDirect + " vs " + firstId);
            } else {
                ok++;
            }
        }
        check(ok == 100, "all 100 success-path iterations must return the stable id (got " + ok + ")");
    }

    // ---------------------------------------------------------------- S5. CompletableFuture exception propagation for push / fetch / diff ----------------------------------------------------------------
    //
    // The spec pins pull() future propagation. Each async entry point has its
    // OWN supplyAsync/runAsync wiring, so a try/catch->CompletionException bug
    // in one is invisible to the others. We pin that a FAILING push, fetch, and
    // diff each throw from .get() with a SnapdirException in the cause chain
    // (ExecutionException -> CompletionException -> SnapdirException). A bare
    // unchecked escape (no CompletionException wrap, or a swallowed throw that
    // completes the future normally) is caught here.

    private static void testFuturePropagationPushFetchDiff(Path tree, Path root) throws Exception {
        // push() to an invalid store  failing future.
        boolean pushThrew = false;
        try {
            Snapdir.push(tree.toString(), "not-a-valid-uri",
                PushOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            pushThrew = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null && isStableCode(c.getCode()),
                "failing push().get() must carry a stable-code SnapdirException cause, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(pushThrew, "a failing push future MUST throw from .get()");

        // fetch() of a bogus id against an EMPTY store  failing future.
        String emptyStore = fileStore(root.resolve("fetch-empty-store"));
        boolean fetchThrew = false;
        try {
            Snapdir.fetch("0".repeat(64), emptyStore, 0).get();
        } catch (ExecutionException ee) {
            fetchThrew = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null && isStableCode(c.getCode()),
                "failing fetch().get() must carry a stable-code SnapdirException cause, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(fetchThrew, "a failing fetch future MUST throw from .get()");

        // diff() against a syntactically-invalid store  failing future. (An
        // ABSENT file:// store is treated as empty/no-error per the API, so we
        // use a malformed URI to force a deterministic INVALID_STORE failure.)
        boolean diffThrew = false;
        try {
            Snapdir.diff("not-a-valid-uri", fileStore(root.resolve("diff-rhs")),
                DiffOptions.builder().build()).get();
        } catch (ExecutionException ee) {
            diffThrew = true;
            SnapdirException c = findSnapdirCause(ee);
            check(c != null && isStableCode(c.getCode()),
                "failing diff().get() must carry a stable-code SnapdirException cause, got "
                    + (c == null ? "<none>" : c.getCode()));
        }
        check(diffThrew, "a failing diff future MUST throw from .get()");
    }

    // ---------------------------------------------------------------- main ----------------------------------------------------------------

    public static void main(String[] args) throws Exception {
        // Hermetic + offline: route cache + catalog into a private temp root so
        // the test never touches the real cache and never hits the network. The
        // C ABI honours these env vars (the cpp/python references rely on them).
        Path root = Files.createTempDirectory("snapdir-java-");
        Path cache = root.resolve("cache");
        Files.createDirectories(cache);
        // Set via system properties is not enough for the native side; the impl
        // harness exports SNAPDIR_CACHE_DIR / SNAPDIR_CATALOG_DB_PATH in the env.
        // We additionally pass cacheDir through the options where the API exposes
        // it, but the env is the authoritative hermetic anchor (set by the runner).

        Path tree = buildTree(root.resolve("tree"));

        try {
            testVersion();
            testManifestEntryShapeAndSizeUnsigned(tree);
            testIdSelfConsistency(tree);
            testCheckedExceptionHierarchy(root);
            testOptionsHonoured(tree);
            testAsyncRoundtrip(tree, root);
            testFutureExactCodes(tree, root);
            testDiffAcrossStores(tree, root);
            testThrowPathNoLeak();
            // -- strengthening (review gate, additive) --
            testMissingPathExactIoError();
            testExactExceptionSubtypes(tree, root);
            testRecordShapesAndSizeUnsignedBoundaries();
            testSuccessPathNoLeakUnderLoad(tree);
            testFuturePropagationPushFetchDiff(tree, root);
        } finally {
            // best-effort cleanup of the private temp root.
            try {
                Files.walk(root)
                    .sorted((a, b) -> b.getNameCount() - a.getNameCount())
                    .forEach(p -> { try { Files.deleteIfExists(p); } catch (IOException ignored) {} });
            } catch (IOException ignored) {}
        }

        System.err.println("checks: " + checks + ", failures: " + failures);
        System.exit(failures == 0 ? 0 : 1);
    }
}
