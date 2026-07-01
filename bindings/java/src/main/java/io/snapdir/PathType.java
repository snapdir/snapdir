package io.snapdir;

/**
 * The type of a manifest entry, corresponding to the first column of each
 * manifest line ({@code TYPE PERM CHECKSUM SIZE PATH}).
 */
public enum PathType {
    /** A regular file ({@code F}). */
    FILE,
    /** A directory ({@code D}). */
    DIRECTORY,
    /** A symbolic link ({@code L}). */
    SYMLINK;

    /**
     * Returns the PathType for the single-character type indicator from the
     * manifest ({@code 'F'}, {@code 'D'}, or {@code 'L'}).
     *
     * @param indicator manifest type character
     * @return corresponding PathType
     * @throws IllegalArgumentException if the indicator is not recognised
     */
    public static PathType fromChar(char indicator) {
        switch (indicator) {
            case 'F': return FILE;
            case 'D': return DIRECTORY;
            case 'L': return SYMLINK;
            default:
                throw new IllegalArgumentException("Unknown PathType indicator: " + indicator);
        }
    }
}
