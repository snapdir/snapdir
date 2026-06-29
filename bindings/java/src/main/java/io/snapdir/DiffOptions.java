package io.snapdir;

/**
 * Options controlling the behaviour of {@link Snapdir#diff}.
 */
public final class DiffOptions {

    /**
     * Optional 64-hex snapshot id; {@code null} = A-to-B cross-store diff
     * (the most common usage). Pass the same store on both sides for a self-diff.
     */
    private final String snapshotId;

    /** If {@code true}, unchanged entries are included in the output. */
    private final boolean includeUnchanged;

    /**
     * Conflict policy: {@code "error"} or {@code "last-wins"};
     * {@code null} = {@code "error"}.
     */
    private final String onConflict;

    private DiffOptions(Builder b) {
        this.snapshotId       = b.snapshotId;
        this.includeUnchanged = b.includeUnchanged;
        this.onConflict       = b.onConflict;
    }

    /**
     * Returns the snapshot id filter, or {@code null}.
     *
     * @return optional 64-hex snapshot id, or {@code null} for a cross-store diff
     */
    public String snapshotId()       { return snapshotId; }

    /**
     * Returns {@code true} if unchanged entries should be included.
     *
     * @return {@code true} when unchanged entries are included in diff output
     */
    public boolean includeUnchanged() { return includeUnchanged; }

    /**
     * Returns the on-conflict policy string, or {@code null}.
     *
     * @return conflict policy ({@code "error"} or {@code "last-wins"}), or {@code null}
     */
    public String onConflict()       { return onConflict; }

    /**
     * Returns a new builder with all defaults.
     *
     * @return a fresh {@link Builder}
     */
    public static Builder builder() { return new Builder(); }

    /** Builder for {@link DiffOptions}. */
    public static final class Builder {
        private String  snapshotId;
        private boolean includeUnchanged;
        private String  onConflict;

        private Builder() {}

        /**
         * Optional 64-hex snapshot id filter.
         *
         * @param snapshotId 64-hex snapshot id, or {@code null} for a cross-store diff
         * @return this builder
         */
        public Builder snapshotId(String snapshotId)           { this.snapshotId = snapshotId; return this; }

        /**
         * Include unchanged entries in the diff result.
         *
         * @param includeUnchanged {@code true} to include entries with no changes
         * @return this builder
         */
        public Builder includeUnchanged(boolean includeUnchanged) { this.includeUnchanged = includeUnchanged; return this; }

        /**
         * Conflict policy ({@code "error"} or {@code "last-wins"}).
         *
         * @param onConflict conflict resolution policy string, or {@code null} for {@code "error"}
         * @return this builder
         */
        public Builder onConflict(String onConflict)           { this.onConflict = onConflict; return this; }

        /**
         * Builds and returns the {@link DiffOptions}.
         *
         * @return a new {@link DiffOptions} with the configured values
         */
        public DiffOptions build() { return new DiffOptions(this); }
    }
}
