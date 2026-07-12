package io.snapdir;

/**
 * Byte-size counts for a snapshot or the whole store.
 *
 * <p>Objects are stored uncompressed, so {@link #physicalBytes} equals the
 * on-disk {@code .objects/} byte total.
 *
 * <p>All fields are {@code long} but logically unsigned {@code uint64}: use
 * {@link #physicalBytesUnsigned()} (and its siblings) for decimal strings when
 * values can exceed {@link Long#MAX_VALUE}.
 *
 * @param logicalBytes  sum of every file's size, duplicates counted
 * @param physicalBytes sum of size over UNIQUE checksums (deduplicated footprint)
 * @param files         count of file entries
 * @param objects       count of distinct content objects (unique checksums)
 */
public record SizeStats(long logicalBytes, long physicalBytes, long files, long objects) {

    /** Unsigned decimal string for {@link #logicalBytes}. */
    public String logicalBytesUnsigned() {
        return Long.toUnsignedString(logicalBytes);
    }

    /** Unsigned decimal string for {@link #physicalBytes}. */
    public String physicalBytesUnsigned() {
        return Long.toUnsignedString(physicalBytes);
    }

    /** Unsigned decimal string for {@link #files}. */
    public String filesUnsigned() {
        return Long.toUnsignedString(files);
    }

    /** Unsigned decimal string for {@link #objects}. */
    public String objectsUnsigned() {
        return Long.toUnsignedString(objects);
    }
}
