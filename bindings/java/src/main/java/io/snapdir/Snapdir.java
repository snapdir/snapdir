package io.snapdir;

import io.snapdir.internal.SnapdirNative;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executor;
import java.util.concurrent.ForkJoinPool;

/**
 * Static-factory API for the snapdir native library.
 *
 * <p>All snapshot operations are exposed as static methods. Synchronous
 * operations ({@link #version}, {@link #manifest}, {@link #id},
 * {@link #idFromManifest}) throw checked {@link SnapdirException} on failure.
 * I/O-bound operations ({@link #push}, {@link #pull}, {@link #fetch},
 * {@link #diff}) return a {@link CompletableFuture} that runs the blocking
 * native call on the common pool (or a caller-supplied {@link Executor}).
 *
 * <p>The native library is initialised lazily via {@link SnapdirNative#init()}
 * on the first call to any method. {@code snapdir_init()} is idempotent and
 * safe to call multiple times.
 *
 * <h2>Example</h2>
 * <pre>{@code
 * // Synchronous manifest + id
 * Manifest m = Snapdir.manifest("/path/to/dir", ManifestOptions.builder().noFollow(true).build());
 * String id  = Snapdir.idFromManifest(m);
 *
 * // Async push
 * Snapdir.push("/path/to/dir", "file:///tmp/store", PushOptions.builder().build())
 *        .thenAccept(snapshotId -> System.out.println("pushed: " + snapshotId))
 *        .join();
 * }</pre>
 */
public final class Snapdir {

    private Snapdir() {}

    // -- Synchronous operations --------------------------------------------------

    /**
     * Returns the snapdir-api version string (e.g. {@code "1.10.0"}).
     *
     * <p>The underlying C string has static lifetime and is never freed.
     *
     * @return version string
     */
    public static String version() {
        SnapdirNative.init();
        return SnapdirNative.version();
    }

    /**
     * Walks {@code path} and returns the parsed directory manifest.
     *
     * @param path directory path
     * @param opts walk options (all fields default when {@code null})
     * @return parsed {@link Manifest}
     * @throws SnapdirException on C ABI failure
     */
    public static Manifest manifest(String path, ManifestOptions opts) throws SnapdirException {
        SnapdirNative.init();
        ManifestOptions o = opts != null ? opts : ManifestOptions.builder().build();
        String raw = SnapdirNative.manifest(
            path,
            o.exclude(),
            o.walkJobs(),
            o.absolute(),
            o.noFollow(),
            o.checksumBin(),
            o.cacheDir(),
            o.catalog()
        );
        return Manifest.parse(raw);
    }

    /**
     * Computes the snapshot id for the directory at {@code path}.
     *
     * <p>When {@code opts.noFollow()} or {@code opts.absolute()} are set the id
     * is derived via manifest -> {@link #idFromManifest} because
     * {@code snapdir_id} does not expose those parameters. For the common case
     * the fast path via {@code snapdir_id} is used directly.
     *
     * @param path directory path
     * @param opts walk options (all fields default when {@code null})
     * @return 64-char lowercase hex BLAKE3 snapshot id
     * @throws SnapdirException on C ABI failure
     */
    public static String id(String path, ManifestOptions opts) throws SnapdirException {
        SnapdirNative.init();
        ManifestOptions o = opts != null ? opts : ManifestOptions.builder().build();
        if (o.noFollow() || o.absolute()) {
            // Fast path unavailable -- derive from manifest text.
            return idFromManifest(manifest(path, o));
        }
        return SnapdirNative.id(path, o.exclude(), o.walkJobs(), o.cacheDir());
    }

    /**
     * Computes the snapshot id from a previously-computed {@link Manifest}.
     *
     * <p>This is a synchronous, pure operation (no filesystem I/O).
     *
     * @param m manifest previously returned by {@link #manifest}
     * @return 64-char lowercase hex BLAKE3 snapshot id
     * @throws SnapdirException on C ABI failure
     */
    public static String idFromManifest(Manifest m) throws SnapdirException {
        SnapdirNative.init();
        return SnapdirNative.idFromManifestText(m.raw());
    }

    // -- Async operations --------------------------------------------------------

    /**
     * Pushes the directory at {@code sourcePath} to {@code storeUri}.
     *
     * <p>The blocking C call runs on {@link ForkJoinPool#commonPool()}.
     *
     * @param sourcePath source directory path
     * @param storeUri   destination store URI (e.g. {@code "file:///tmp/store"})
     * @param opts       push options (all fields default when {@code null})
     * @return future resolving to the 64-char hex snapshot id
     */
    public static CompletableFuture<String> push(
            String sourcePath,
            String storeUri,
            PushOptions opts
    ) {
        PushOptions o = opts != null ? opts : PushOptions.builder().build();
        return CompletableFuture.supplyAsync(() -> {
            SnapdirNative.init();
            try {
                return SnapdirNative.pushBlocking(
                    sourcePath,
                    o.sourceId(),
                    storeUri,
                    o.jobs(),
                    o.limitRate(),
                    o.maxRetries(),
                    o.cacheDir()
                );
            } catch (SnapdirException e) {
                throw new java.util.concurrent.CompletionException(e);
            }
        });
    }

    /**
     * Pushes the directory at {@code sourcePath} to {@code storeUri}, running
     * the blocking call on the supplied {@code executor}.
     *
     * @param sourcePath source directory path
     * @param storeUri   destination store URI
     * @param opts       push options (all fields default when {@code null})
     * @param executor   executor to run the blocking call on
     * @return future resolving to the 64-char hex snapshot id
     */
    public static CompletableFuture<String> push(
            String sourcePath,
            String storeUri,
            PushOptions opts,
            Executor executor
    ) {
        PushOptions o = opts != null ? opts : PushOptions.builder().build();
        return CompletableFuture.supplyAsync(() -> {
            SnapdirNative.init();
            try {
                return SnapdirNative.pushBlocking(
                    sourcePath,
                    o.sourceId(),
                    storeUri,
                    o.jobs(),
                    o.limitRate(),
                    o.maxRetries(),
                    o.cacheDir()
                );
            } catch (SnapdirException e) {
                throw new java.util.concurrent.CompletionException(e);
            }
        }, executor);
    }

    /**
     * Pulls a snapshot from {@code storeUri} and materializes it into {@code destPath}.
     *
     * <p>The blocking C call runs on {@link ForkJoinPool#commonPool()}.
     *
     * @param snapshotId 64-hex snapshot id
     * @param storeUri   source store URI
     * @param destPath   destination filesystem path
     * @param opts       pull options (all fields default when {@code null})
     * @return future resolving to {@code null} on success
     */
    public static CompletableFuture<Void> pull(
            String snapshotId,
            String storeUri,
            String destPath,
            PullOptions opts
    ) {
        PullOptions o = opts != null ? opts : PullOptions.builder().build();
        return CompletableFuture.runAsync(() -> {
            SnapdirNative.init();
            try {
                SnapdirNative.pullBlocking(snapshotId, storeUri, destPath,
                    o.deleteExtra(), o.jobs());
            } catch (SnapdirException e) {
                throw new java.util.concurrent.CompletionException(e);
            }
        });
    }

    /**
     * Fetches a snapshot from {@code storeUri} into the local cache.
     *
     * <p>The blocking C call runs on {@link ForkJoinPool#commonPool()}.
     *
     * @param snapshotId 64-hex snapshot id
     * @param storeUri   source store URI
     * @param jobs       max concurrent transfer jobs (0 = default)
     * @return future resolving to {@code null} on success
     */
    public static CompletableFuture<Void> fetch(
            String snapshotId,
            String storeUri,
            int jobs
    ) {
        return CompletableFuture.runAsync(() -> {
            SnapdirNative.init();
            try {
                SnapdirNative.fetchBlocking(snapshotId, storeUri, jobs);
            } catch (SnapdirException e) {
                throw new java.util.concurrent.CompletionException(e);
            }
        });
    }

    /**
     * Computes the difference between two stores.
     *
     * <p>The blocking C call runs on {@link ForkJoinPool#commonPool()}.
     * An absent {@code file://} store is treated as empty (no error).
     *
     * @param fromUri source store URI
     * @param toUri   destination store URI
     * @param opts    diff options (all fields default when {@code null})
     * @return future resolving to a list of {@link DiffEntry} records
     */
    public static CompletableFuture<List<DiffEntry>> diff(
            String fromUri,
            String toUri,
            DiffOptions opts
    ) {
        DiffOptions o = opts != null ? opts : DiffOptions.builder().build();
        return CompletableFuture.supplyAsync(() -> {
            SnapdirNative.init();
            try {
                String json = SnapdirNative.diffJson(
                    fromUri, toUri,
                    o.snapshotId(),
                    o.includeUnchanged(),
                    o.onConflict()
                );
                return parseDiffJson(json);
            } catch (SnapdirException e) {
                throw new java.util.concurrent.CompletionException(e);
            }
        });
    }

    // -- JSON helpers ------------------------------------------------------------

    /**
     * Minimal hand-rolled parse of the diff JSON array returned by the C ABI.
     *
     * <p>Shape: {@code [{"status":"A","path":"./add.txt"}, ...]}.
     * Status is one of {@code "A"}, {@code "D"}, {@code "M"}, {@code "="}.
     * This avoids a JSON library dependency at the cost of generality; it
     * handles the specific shape the C ABI produces.
     */
    static List<DiffEntry> parseDiffJson(String json) {
        List<DiffEntry> entries = new ArrayList<>();
        int pos = 0;
        while (pos < json.length()) {
            int objOpen = json.indexOf('{', pos);
            if (objOpen < 0) break;

            // Find matching closing brace, skipping JSON strings.
            int depth = 0;
            int objClose = -1;
            for (int i = objOpen; i < json.length(); i++) {
                char c = json.charAt(i);
                if (c == '\\') {
                    i++; // skip escaped character
                } else if (c == '"') {
                    // skip string body
                    i++;
                    while (i < json.length()) {
                        char sc = json.charAt(i);
                        if (sc == '\\') i++;
                        else if (sc == '"') break;
                        i++;
                    }
                } else if (c == '{') {
                    depth++;
                } else if (c == '}') {
                    depth--;
                    if (depth == 0) { objClose = i; break; }
                }
            }
            if (objClose < 0) break;

            String obj = json.substring(objOpen, objClose + 1);

            // Extract "status":"X"
            char statusChar = 0;
            int stIdx = obj.indexOf("\"status\":\"");
            if (stIdx >= 0) {
                int valueIdx = stIdx + 10;
                if (valueIdx < obj.length()) statusChar = obj.charAt(valueIdx);
            }

            // Extract "path":"Y" (with simple JSON escape handling)
            String pathStr = null;
            int paIdx = obj.indexOf("\"path\":\"");
            if (paIdx >= 0) {
                int valueStart = paIdx + 7; // points at opening '"' of value
                if (valueStart < obj.length() && obj.charAt(valueStart) == '"') {
                    StringBuilder sb = new StringBuilder();
                    int i = valueStart + 1;
                    while (i < obj.length()) {
                        char c = obj.charAt(i);
                        if (c == '\\' && i + 1 < obj.length()) {
                            char next = obj.charAt(i + 1);
                            switch (next) {
                                case '"': sb.append('"'); break;
                                case '\\': sb.append('\\'); break;
                                case '/': sb.append('/'); break;
                                case 'n': sb.append('\n'); break;
                                case 'r': sb.append('\r'); break;
                                case 't': sb.append('\t'); break;
                                default: sb.append('\\'); sb.append(next); break;
                            }
                            i += 2;
                        } else if (c == '"') {
                            pathStr = sb.toString();
                            break;
                        } else {
                            sb.append(c);
                            i++;
                        }
                    }
                }
            }

            if (statusChar != 0 && pathStr != null && !pathStr.isEmpty()) {
                try {
                    DiffStatus status = DiffStatus.fromChar(statusChar);
                    entries.add(new DiffEntry(status, pathStr));
                } catch (IllegalArgumentException ignored) {
                    // unknown status -- skip
                }
            }
            pos = objClose + 1;
        }
        return Collections.unmodifiableList(entries);
    }
}
