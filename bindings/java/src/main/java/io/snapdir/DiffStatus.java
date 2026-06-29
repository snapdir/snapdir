package io.snapdir;

/**
 * The change status of a single entry in a store diff result, corresponding to
 * the {@code "status"} field in the JSON returned by {@code snapdir_diff_json}.
 */
public enum DiffStatus {
    /** The path was added in the destination ({@code "A"}). */
    ADDED,
    /** The path was deleted in the destination ({@code "D"}). */
    DELETED,
    /** The path was modified ({@code "M"}). */
    MODIFIED,
    /** The path is unchanged ({@code "="}). */
    UNCHANGED;

    /**
     * Returns the DiffStatus for the single-character status code from the diff
     * JSON ({@code 'A'}, {@code 'D'}, {@code 'M'}, or {@code '='}).
     *
     * @param code status character
     * @return corresponding DiffStatus
     * @throws IllegalArgumentException if the code is not recognised
     */
    public static DiffStatus fromChar(char code) {
        switch (code) {
            case 'A': return ADDED;
            case 'D': return DELETED;
            case 'M': return MODIFIED;
            case '=': return UNCHANGED;
            default:
                throw new IllegalArgumentException("Unknown DiffStatus code: " + code);
        }
    }
}
