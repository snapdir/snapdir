package io.snapdir.internal;

import io.snapdir.CatalogException;
import io.snapdir.HashMismatchException;
import io.snapdir.InFluxException;
import io.snapdir.NativeLoader;
import io.snapdir.SnapdirException;
import io.snapdir.StoreException;
import jdk.incubator.foreign.CLinker;
import jdk.incubator.foreign.FunctionDescriptor;
import jdk.incubator.foreign.MemoryAccess;
import jdk.incubator.foreign.MemoryAddress;
import jdk.incubator.foreign.MemoryLayout;
import jdk.incubator.foreign.MemorySegment;
import jdk.incubator.foreign.ResourceScope;
import jdk.incubator.foreign.SymbolLookup;

import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodType;

/**
 * Package-private native layer: binds every C ABI function via JDK-17
 * {@code jdk.incubator.foreign} downcall handles.
 *
 * <p>All public methods are safe: they acquire/release {@link ResourceScope}s
 * around every call, free every returned {@code char*} (or {@code SnapdirError*}),
 * and translate C error out-params into the appropriate {@link SnapdirException}
 * subclass. No memory is leaked even when an exception is thrown.
 *
 * <p>The library is loaded once from the JAR resource via {@link NativeLoader};
 * all handles are resolved once at class-load time.
 *
 * <p><b>JDK 17 incubator API names used here:</b>
 * {@code CLinker.getInstance()}, {@code SymbolLookup.loaderLookup()},
 * {@code linker.downcallHandle(MemoryAddress, MethodType, FunctionDescriptor)},
 * layouts {@code CLinker.C_POINTER}/{@code C_INT}/{@code C_LONG}/{@code C_CHAR},
 * {@code FunctionDescriptor.of(...)} / {@code .ofVoid(...)},
 * {@code CLinker.toJavaString(MemoryAddress)},
 * {@code CLinker.toCString(String, ResourceScope)} (returns MemorySegment; use
 * {@code .address()} to get a {@code MemoryAddress} for passing to C),
 * {@code ResourceScope.newConfinedScope()} (try-with-resources),
 * {@code MemorySegment.allocateNative(layout, scope)},
 * {@code MemoryAccess.getAddress(MemorySegment)}.
 */
@SuppressWarnings({"removal", "preview"})
public final class SnapdirNative {

    // -- Library load ------------------------------------------------------------

    private static final String LIB_PATH;

    static {
        try {
            LIB_PATH = NativeLoader.load();
        } catch (SnapdirException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    // -- CLinker + SymbolLookup --------------------------------------------------

    private static final CLinker LINKER = CLinker.getInstance();
    private static final SymbolLookup LOOKUP = SymbolLookup.loaderLookup();

    // -- Downcall handles --------------------------------------------------------

    // void snapdir_init(void)
    private static final MethodHandle MH_INIT =
        LINKER.downcallHandle(
            sym("snapdir_init"),
            MethodType.methodType(void.class),
            FunctionDescriptor.ofVoid()
        );

    // const char* snapdir_version(void)  -- static lifetime, do NOT free
    private static final MethodHandle MH_VERSION =
        LINKER.downcallHandle(
            sym("snapdir_version"),
            MethodType.methodType(MemoryAddress.class),
            FunctionDescriptor.of(CLinker.C_POINTER)
        );

    // char* snapdir_manifest(path, exclude, walk_jobs, absolute, no_follow,
    //                        checksum_bin, cache_dir, catalog, err_out)
    private static final MethodHandle MH_MANIFEST =
        LINKER.downcallHandle(
            sym("snapdir_manifest"),
            MethodType.methodType(
                MemoryAddress.class,   // return: char*
                MemoryAddress.class,   // path
                MemoryAddress.class,   // exclude (nullable)
                int.class,             // walk_jobs (uint32_t -> int)
                byte.class,            // absolute (bool)
                byte.class,            // no_follow (bool)
                MemoryAddress.class,   // checksum_bin (nullable)
                MemoryAddress.class,   // cache_dir (nullable)
                MemoryAddress.class,   // catalog (nullable)
                MemoryAddress.class    // err_out: SnapdirError**
            ),
            FunctionDescriptor.of(
                CLinker.C_POINTER,     // char*
                CLinker.C_POINTER,     // path
                CLinker.C_POINTER,     // exclude
                CLinker.C_INT,         // walk_jobs
                CLinker.C_CHAR,        // absolute
                CLinker.C_CHAR,        // no_follow
                CLinker.C_POINTER,     // checksum_bin
                CLinker.C_POINTER,     // cache_dir
                CLinker.C_POINTER,     // catalog
                CLinker.C_POINTER      // err_out
            )
        );

    // char* snapdir_id(path, exclude, walk_jobs, cache_dir, err_out)
    private static final MethodHandle MH_ID =
        LINKER.downcallHandle(
            sym("snapdir_id"),
            MethodType.methodType(
                MemoryAddress.class,
                MemoryAddress.class,   // path
                MemoryAddress.class,   // exclude
                int.class,             // walk_jobs
                MemoryAddress.class,   // cache_dir
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_INT,
                CLinker.C_POINTER,
                CLinker.C_POINTER
            )
        );

    // char* snapdir_id_from_manifest_text(manifest_text, err_out)
    private static final MethodHandle MH_ID_FROM_MANIFEST =
        LINKER.downcallHandle(
            sym("snapdir_id_from_manifest_text"),
            MethodType.methodType(
                MemoryAddress.class,
                MemoryAddress.class,   // manifest_text
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER
            )
        );

    // char* snapdir_push_blocking(source_path, source_id, store_uri, jobs,
    //                             limit_rate, max_retries, cache_dir, err_out)
    private static final MethodHandle MH_PUSH =
        LINKER.downcallHandle(
            sym("snapdir_push_blocking"),
            MethodType.methodType(
                MemoryAddress.class,
                MemoryAddress.class,   // source_path
                MemoryAddress.class,   // source_id
                MemoryAddress.class,   // store_uri
                int.class,             // jobs
                MemoryAddress.class,   // limit_rate
                int.class,             // max_retries
                MemoryAddress.class,   // cache_dir
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_INT,
                CLinker.C_POINTER,
                CLinker.C_INT,
                CLinker.C_POINTER,
                CLinker.C_POINTER
            )
        );

    // int snapdir_pull_blocking(id, store_uri, dest_path, delete_extra, jobs, err_out)
    private static final MethodHandle MH_PULL =
        LINKER.downcallHandle(
            sym("snapdir_pull_blocking"),
            MethodType.methodType(
                int.class,
                MemoryAddress.class,   // id
                MemoryAddress.class,   // store_uri
                MemoryAddress.class,   // dest_path
                byte.class,            // delete_extra
                int.class,             // jobs
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_INT,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_CHAR,
                CLinker.C_INT,
                CLinker.C_POINTER
            )
        );

    // int snapdir_fetch_blocking(id, store_uri, jobs, err_out)
    private static final MethodHandle MH_FETCH =
        LINKER.downcallHandle(
            sym("snapdir_fetch_blocking"),
            MethodType.methodType(
                int.class,
                MemoryAddress.class,   // id
                MemoryAddress.class,   // store_uri
                int.class,             // jobs
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_INT,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_INT,
                CLinker.C_POINTER
            )
        );

    // char* snapdir_diff_json(from_uris, to_uris, id, include_unchanged,
    //                         on_conflict, err_out)
    // Note: from_uris / to_uris are NULL-terminated char** arrays.
    private static final MethodHandle MH_DIFF =
        LINKER.downcallHandle(
            sym("snapdir_diff_json"),
            MethodType.methodType(
                MemoryAddress.class,
                MemoryAddress.class,   // from_uris (char**)
                MemoryAddress.class,   // to_uris (char**)
                MemoryAddress.class,   // id (nullable)
                byte.class,            // include_unchanged
                MemoryAddress.class,   // on_conflict (nullable)
                MemoryAddress.class    // err_out
            ),
            FunctionDescriptor.of(
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_POINTER,
                CLinker.C_CHAR,
                CLinker.C_POINTER,
                CLinker.C_POINTER
            )
        );

    // void snapdir_string_free(char*)
    private static final MethodHandle MH_STRING_FREE =
        LINKER.downcallHandle(
            sym("snapdir_string_free"),
            MethodType.methodType(void.class, MemoryAddress.class),
            FunctionDescriptor.ofVoid(CLinker.C_POINTER)
        );

    // void snapdir_error_free(SnapdirError*)
    private static final MethodHandle MH_ERROR_FREE =
        LINKER.downcallHandle(
            sym("snapdir_error_free"),
            MethodType.methodType(void.class, MemoryAddress.class),
            FunctionDescriptor.ofVoid(CLinker.C_POINTER)
        );

    // const char* snapdir_error_code(const SnapdirError*) -- borrowed, do NOT free
    private static final MethodHandle MH_ERROR_CODE =
        LINKER.downcallHandle(
            sym("snapdir_error_code"),
            MethodType.methodType(MemoryAddress.class, MemoryAddress.class),
            FunctionDescriptor.of(CLinker.C_POINTER, CLinker.C_POINTER)
        );

    // const char* snapdir_error_message(const SnapdirError*) -- borrowed, do NOT free
    private static final MethodHandle MH_ERROR_MESSAGE =
        LINKER.downcallHandle(
            sym("snapdir_error_message"),
            MethodType.methodType(MemoryAddress.class, MemoryAddress.class),
            FunctionDescriptor.of(CLinker.C_POINTER, CLinker.C_POINTER)
        );

    // -- Symbol lookup helper ----------------------------------------------------

    private static MemoryAddress sym(String name) {
        return LOOKUP.lookup(name)
            .orElseThrow(() -> new UnsatisfiedLinkError(
                "Symbol not found in libsnapdir_ffi: " + name));
    }

    // -- Public API --------------------------------------------------------------

    private SnapdirNative() {}

    /**
     * Calls {@code snapdir_init()}.  Idempotent; safe to call multiple times.
     */
    public static void init() {
        try {
            MH_INIT.invoke();
        } catch (Throwable t) {
            throw new RuntimeException("snapdir_init failed", t);
        }
    }

    /**
     * Returns the snapdir version string (static lifetime; never freed).
     *
     * @return snapdir version string (e.g. {@code "1.10.0"})
     */
    public static String version() {
        try {
            MemoryAddress addr = (MemoryAddress) MH_VERSION.invoke();
            return CLinker.toJavaString(addr);
        } catch (Throwable t) {
            throw new RuntimeException("snapdir_version failed", t);
        }
    }

    /**
     * Walks {@code path} and returns the manifest text.
     *
     * @param path        directory path (required)
     * @param exclude     exclusion regex or {@code null}
     * @param walkJobs    parallel job count (0 = auto)
     * @param absolute    emit absolute paths
     * @param noFollow    do not follow symlinks
     * @param checksumBin checksum algorithm or {@code null}
     * @param cacheDir    cache directory override or {@code null}
     * @param catalog     catalog adapter or {@code null}
     * @return manifest text
     * @throws SnapdirException on C ABI failure
     */
    public static String manifest(
            String path,
            String exclude,
            int walkJobs,
            boolean absolute,
            boolean noFollow,
            String checksumBin,
            String cacheDir,
            String catalog
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            // err_out: SnapdirError** -- a single pointer-sized slot
            MemorySegment errOutSeg = MemorySegment.allocateNative(
                CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cPath        = toCString(path, scope);
            MemoryAddress cExclude     = toCStringOrNull(exclude, scope);
            MemoryAddress cChecksumBin = toCStringOrNull(checksumBin, scope);
            MemoryAddress cCacheDir    = toCStringOrNull(cacheDir, scope);
            MemoryAddress cCatalog     = toCStringOrNull(catalog, scope);

            MemoryAddress result = (MemoryAddress) MH_MANIFEST.invoke(
                cPath,
                cExclude,
                walkJobs,
                boolToByte(absolute),
                boolToByte(noFollow),
                cChecksumBin,
                cCacheDir,
                cCatalog,
                errOutSeg.address()
            );

            MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
            if (result.equals(MemoryAddress.NULL)) {
                throw takeError(errPtr);
            }
            try {
                return CLinker.toJavaString(result);
            } finally {
                freeString(result);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_manifest invocation failed: " + t.getMessage(), t);
        }
    }

    /**
     * Computes the snapshot id for the directory at {@code path}.
     *
     * @param path     directory path
     * @param exclude  exclusion regex or {@code null}
     * @param walkJobs parallel job count (0 = auto)
     * @param cacheDir cache directory override or {@code null}
     * @return 64-char lowercase hex snapshot id
     * @throws SnapdirException on C ABI failure
     */
    public static String id(
            String path,
            String exclude,
            int walkJobs,
            String cacheDir
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cPath    = toCString(path, scope);
            MemoryAddress cExclude = toCStringOrNull(exclude, scope);
            MemoryAddress cCacheDir = toCStringOrNull(cacheDir, scope);

            MemoryAddress result = (MemoryAddress) MH_ID.invoke(
                cPath, cExclude, walkJobs, cCacheDir, errOutSeg.address());

            MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
            if (result.equals(MemoryAddress.NULL)) {
                throw takeError(errPtr);
            }
            try {
                return CLinker.toJavaString(result);
            } finally {
                freeString(result);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_id invocation failed: " + t.getMessage(), t);
        }
    }

    /**
     * Computes the snapshot id from previously-computed manifest text.
     *
     * @param manifestText manifest text
     * @return 64-char lowercase hex snapshot id
     * @throws SnapdirException on C ABI failure
     */
    public static String idFromManifestText(String manifestText) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cText = toCString(manifestText, scope);

            MemoryAddress result = (MemoryAddress) MH_ID_FROM_MANIFEST.invoke(
                cText, errOutSeg.address());

            MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
            if (result.equals(MemoryAddress.NULL)) {
                throw takeError(errPtr);
            }
            try {
                return CLinker.toJavaString(result);
            } finally {
                freeString(result);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_id_from_manifest_text failed: " + t.getMessage(), t);
        }
    }

    /**
     * Pushes the directory at {@code sourcePath} to {@code storeUri} and returns
     * the 64-char snapshot id.
     *
     * @param sourcePath  directory to push (XOR with sourceId)
     * @param sourceId    pre-staged snapshot id or {@code null}
     * @param storeUri    destination store URI
     * @param jobs        max concurrent transfer jobs (0 = default)
     * @param limitRate   bandwidth cap string or {@code null}
     * @param maxRetries  max retries per object (0 = default 5)
     * @param cacheDir    cache directory override or {@code null}
     * @return 64-char snapshot id
     * @throws SnapdirException on C ABI failure
     */
    public static String pushBlocking(
            String sourcePath,
            String sourceId,
            String storeUri,
            int jobs,
            String limitRate,
            int maxRetries,
            String cacheDir
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cSourcePath = toCStringOrNull(sourcePath, scope);
            MemoryAddress cSourceId   = toCStringOrNull(sourceId, scope);
            MemoryAddress cStoreUri   = toCString(storeUri, scope);
            MemoryAddress cLimitRate  = toCStringOrNull(limitRate, scope);
            MemoryAddress cCacheDir   = toCStringOrNull(cacheDir, scope);

            MemoryAddress result = (MemoryAddress) MH_PUSH.invoke(
                cSourcePath, cSourceId, cStoreUri,
                jobs, cLimitRate, maxRetries, cCacheDir,
                errOutSeg.address());

            MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
            if (result.equals(MemoryAddress.NULL)) {
                throw takeError(errPtr);
            }
            try {
                return CLinker.toJavaString(result);
            } finally {
                freeString(result);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_push_blocking failed: " + t.getMessage(), t);
        }
    }

    /**
     * Pulls a snapshot from {@code storeUri} and materializes it into {@code destPath}.
     *
     * @param snapshotId  64-hex snapshot id
     * @param storeUri    source store URI
     * @param destPath    destination filesystem path
     * @param deleteExtra delete destination files absent from the snapshot
     * @param jobs        max concurrent jobs (0 = default)
     * @throws SnapdirException on C ABI failure
     */
    public static void pullBlocking(
            String snapshotId,
            String storeUri,
            String destPath,
            boolean deleteExtra,
            int jobs
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cId       = toCString(snapshotId, scope);
            MemoryAddress cStore    = toCString(storeUri, scope);
            MemoryAddress cDestPath = toCString(destPath, scope);

            int rc = (int) MH_PULL.invoke(
                cId, cStore, cDestPath,
                boolToByte(deleteExtra), jobs,
                errOutSeg.address());

            if (rc != 0) {
                MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
                throw takeError(errPtr);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_pull_blocking failed: " + t.getMessage(), t);
        }
    }

    /**
     * Fetches a snapshot from {@code storeUri} into the local cache.
     *
     * @param snapshotId 64-hex snapshot id
     * @param storeUri   source store URI
     * @param jobs       max concurrent jobs (0 = default)
     * @throws SnapdirException on C ABI failure
     */
    public static void fetchBlocking(
            String snapshotId,
            String storeUri,
            int jobs
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cId    = toCString(snapshotId, scope);
            MemoryAddress cStore = toCString(storeUri, scope);

            int rc = (int) MH_FETCH.invoke(cId, cStore, jobs, errOutSeg.address());

            if (rc != 0) {
                MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
                throw takeError(errPtr);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_fetch_blocking failed: " + t.getMessage(), t);
        }
    }

    /**
     * Computes the diff between two stores and returns the raw JSON string.
     *
     * @param fromUri         source store URI
     * @param toUri           destination store URI
     * @param snapshotId      optional snapshot id filter or {@code null}
     * @param includeUnchanged include unchanged entries in the output
     * @param onConflict      conflict policy or {@code null} (= {@code "error"})
     * @return JSON array string of diff entries
     * @throws SnapdirException on C ABI failure
     */
    public static String diffJson(
            String fromUri,
            String toUri,
            String snapshotId,
            boolean includeUnchanged,
            String onConflict
    ) throws SnapdirException {
        try (ResourceScope scope = ResourceScope.newConfinedScope()) {
            MemorySegment errOutSeg = MemorySegment.allocateNative(CLinker.C_POINTER, scope);
            MemoryAccess.setAddress(errOutSeg, MemoryAddress.NULL);

            MemoryAddress cFrom       = toCString(fromUri, scope);
            MemoryAddress cTo         = toCString(toUri, scope);
            MemoryAddress cSnapshotId = toCStringOrNull(snapshotId, scope);
            MemoryAddress cOnConflict = toCStringOrNull(onConflict, scope);

            // Build NULL-terminated pointer arrays for from_uris and to_uris.
            // Each array is 2 pointer-slots: [ptr, NULL].
            MemoryLayout ptrLayout = CLinker.C_POINTER;
            MemorySegment fromArr = MemorySegment.allocateNative(
                MemoryLayout.sequenceLayout(2, ptrLayout), scope);
            MemorySegment toArr = MemorySegment.allocateNative(
                MemoryLayout.sequenceLayout(2, ptrLayout), scope);

            // Slot 0 = the URI pointer; slot 1 = NULL sentinel.
            MemoryAccess.setAddressAtIndex(fromArr, 0, cFrom);
            MemoryAccess.setAddressAtIndex(fromArr, 1, MemoryAddress.NULL);
            MemoryAccess.setAddressAtIndex(toArr, 0, cTo);
            MemoryAccess.setAddressAtIndex(toArr, 1, MemoryAddress.NULL);

            MemoryAddress result = (MemoryAddress) MH_DIFF.invoke(
                fromArr.address(), toArr.address(),
                cSnapshotId,
                boolToByte(includeUnchanged),
                cOnConflict,
                errOutSeg.address());

            MemoryAddress errPtr = MemoryAccess.getAddress(errOutSeg);
            if (result.equals(MemoryAddress.NULL)) {
                throw takeError(errPtr);
            }
            try {
                return CLinker.toJavaString(result);
            } finally {
                freeString(result);
            }
        } catch (SnapdirException e) {
            throw e;
        } catch (Throwable t) {
            throw new SnapdirException("INTERNAL", "snapdir_diff_json failed: " + t.getMessage(), t);
        }
    }

    // -- Private helpers ---------------------------------------------------------

    /**
     * Converts a Java String to a NUL-terminated C string within the given scope.
     * Returns the address of the allocated segment.
     */
    private static MemoryAddress toCString(String s, ResourceScope scope) {
        return CLinker.toCString(s, scope).address();
    }

    /**
     * Like {@link #toCString} but returns {@link MemoryAddress#NULL} when
     * {@code s} is {@code null}, matching the C ABI optional-string convention.
     */
    private static MemoryAddress toCStringOrNull(String s, ResourceScope scope) {
        if (s == null) return MemoryAddress.NULL;
        return toCString(s, scope);
    }

    /**
     * Converts a Java boolean to the single-byte representation expected by
     * the C ABI (C {@code bool}: 1 = true, 0 = false).
     */
    private static byte boolToByte(boolean b) {
        return b ? (byte) 1 : (byte) 0;
    }

    /**
     * Frees a {@code char*} returned by a snapdir C function via
     * {@code snapdir_string_free}.  Calling with {@link MemoryAddress#NULL} is
     * a safe no-op (the C ABI guarantees it).
     */
    private static void freeString(MemoryAddress ptr) {
        try {
            MH_STRING_FREE.invoke(ptr);
        } catch (Throwable t) {
            // Should never happen; swallow silently -- the C fn accepts NULL.
        }
    }

    /**
     * Reads the code and message from a {@code SnapdirError*}, frees the error
     * object, and returns the appropriate checked exception.
     *
     * <p>If {@code errPtr} is {@link MemoryAddress#NULL} (which indicates a
     * contract violation from the C side), an {@code INTERNAL} exception is returned.
     *
     * @param errPtr pointer to a live SnapdirError (will be freed)
     * @return the appropriate SnapdirException subclass
     */
    private static SnapdirException takeError(MemoryAddress errPtr) {
        if (errPtr == null || errPtr.equals(MemoryAddress.NULL)) {
            return new SnapdirException("INTERNAL",
                "C ABI returned failure with no error object set");
        }
        String code;
        String message;
        try {
            MemoryAddress codeAddr = (MemoryAddress) MH_ERROR_CODE.invoke(errPtr);
            MemoryAddress msgAddr  = (MemoryAddress) MH_ERROR_MESSAGE.invoke(errPtr);
            code    = codeAddr.equals(MemoryAddress.NULL) ? "INTERNAL"
                                                          : CLinker.toJavaString(codeAddr);
            message = msgAddr.equals(MemoryAddress.NULL)  ? "(no message)"
                                                          : CLinker.toJavaString(msgAddr);
        } catch (Throwable t) {
            code    = "INTERNAL";
            message = "Failed to read SnapdirError: " + t.getMessage();
        } finally {
            try {
                MH_ERROR_FREE.invoke(errPtr);
            } catch (Throwable ignored) {}
        }
        return mapException(code, message);
    }

    /**
     * Maps a stable ABI code string to the appropriate exception subclass.
     */
    private static SnapdirException mapException(String code, String message) {
        switch (code) {
            case "HASH_MISMATCH":  return new HashMismatchException(message);
            case "STORE_ERROR":    return new StoreException(message);
            case "IN_FLUX":        return new InFluxException(message);
            case "CATALOG_ERROR":  return new CatalogException(message);
            default:               return new SnapdirException(code, message);
        }
    }
}
