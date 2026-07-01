package io.snapdir;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * A parsed snapdir manifest returned by {@link Snapdir#manifest}.
 *
 * <p>The manifest text format is one entry per line: {@code TYPE PERM CHECKSUM SIZE PATH}.
 * Blank lines and lines beginning with {@code '#'} are ignored.
 *
 * @param raw     full manifest text exactly as returned by the C library
 * @param entries parsed list of manifest entries (unmodifiable)
 */
public record Manifest(String raw, List<ManifestEntry> entries) {

    /**
     * Parses manifest text into a {@code Manifest} record.
     *
     * <p>Blank lines and lines beginning with {@code '#'} are skipped.
     * Lines with fewer than 5 space-delimited tokens are skipped silently.
     *
     * @param text manifest text (as returned by the C ABI or read from a file)
     * @return parsed Manifest
     */
    public static Manifest parse(String text) {
        List<ManifestEntry> entries = new ArrayList<>();
        for (String line : text.split("\n", -1)) {
            // Strip trailing CR for CRLF inputs.
            if (line.endsWith("\r")) {
                line = line.substring(0, line.length() - 1);
            }
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            // Split into at most 5 parts: TYPE PERM CHECKSUM SIZE PATH
            String[] parts = line.split(" ", 5);
            if (parts.length < 5) {
                continue;
            }
            PathType type;
            try {
                type = PathType.fromChar(parts[0].charAt(0));
            } catch (IllegalArgumentException e) {
                continue;
            }
            int perm;
            try {
                perm = Integer.parseInt(parts[1], 8);
            } catch (NumberFormatException e) {
                continue;
            }
            String checksum = parts[2];
            long size;
            try {
                // parseUnsignedLong handles the full uint64 range; the stored
                // long may be negative for files > Long.MAX_VALUE bytes but
                // toUnsignedString() recovers the correct decimal string.
                size = Long.parseUnsignedLong(parts[3]);
            } catch (NumberFormatException e) {
                continue;
            }
            String path = parts[4];
            entries.add(new ManifestEntry(type, perm, checksum, size, path));
        }
        return new Manifest(text, Collections.unmodifiableList(entries));
    }
}
