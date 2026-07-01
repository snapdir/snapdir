package io.snapdir;

/**
 * A single entry from a {@link Snapdir#diff} result.
 *
 * <p>Each entry corresponds to one JSON object in the array returned by
 * {@code snapdir_diff_json}: {@code {"status":"A","path":"./add.txt"}}.
 *
 * @param status change status (ADDED, DELETED, MODIFIED, or UNCHANGED)
 * @param path   entry path (e.g. {@code "./subdir/file.txt"})
 */
public record DiffEntry(DiffStatus status, String path) {}
