package io.snapdir;

/**
 * A single parsed line from a snapdir manifest.
 *
 * <p>The manifest format is: {@code TYPE PERM CHECKSUM SIZE PATH}
 * where TYPE is {@code F}, {@code D}, or {@code L}; PERM is octal mode bits;
 * CHECKSUM is the 64-char BLAKE3 hex hash (or {@code "-"} for directories);
 * SIZE is the byte count; and PATH is the entry path.
 *
 * <p><b>Size note:</b> {@code size} is stored as a signed {@code long}. For
 * very large files (> 9,223,372,036,854,775,807 bytes) the value will be negative
 * when interpreted as a signed long. Use {@link #sizeUnsigned()} to get the
 * correct decimal string representation in such cases.
 *
 * @param type     entry type (FILE, DIRECTORY, or SYMLINK)
 * @param perm     POSIX mode bits in octal (e.g. 0644 decimal = 420)
 * @param checksum 64-char BLAKE3 hex hash, or {@code "-"} for directories
 * @param size     byte count as a signed long (see note above)
 * @param path     entry path as recorded in the manifest
 */
public record ManifestEntry(PathType type, int perm, String checksum, long size, String path) {

    /**
     * Returns the size as an unsigned decimal string.
     *
     * <p>Use this in preference to {@code Long.toString(size())} when the size
     * could be larger than {@link Long#MAX_VALUE} -- Java's {@code long} is
     * signed, so values above 2^63-1 wrap to negative.
     *
     * @return unsigned decimal representation of the size
     */
    public String sizeUnsigned() {
        return Long.toUnsignedString(size);
    }
}
