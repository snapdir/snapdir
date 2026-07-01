package io.snapdir;

/**
 * Options controlling the behaviour of {@link Snapdir#pull}.
 */
public final class PullOptions {

    /**
     * If {@code true}, destination files absent from the snapshot are deleted.
     */
    private final boolean deleteExtra;

    /** Max concurrent transfer jobs; {@code 0} = default. */
    private final int jobs;

    private PullOptions(Builder b) {
        this.deleteExtra = b.deleteExtra;
        this.jobs        = b.jobs;
    }

    /**
     * Returns {@code true} if extra destination files should be deleted.
     *
     * @return {@code true} when files at the destination absent from the snapshot are removed
     */
    public boolean deleteExtra() { return deleteExtra; }

    /**
     * Returns the max concurrent transfer jobs (0 = default).
     *
     * @return maximum number of parallel transfer jobs, or {@code 0} for the default
     */
    public int jobs()            { return jobs; }

    /**
     * Returns a new builder with all defaults.
     *
     * @return a fresh {@link Builder}
     */
    public static Builder builder() { return new Builder(); }

    /** Builder for {@link PullOptions}. */
    public static final class Builder {
        private boolean deleteExtra;
        private int     jobs;

        private Builder() {}

        /**
         * Delete destination files absent from the snapshot.
         *
         * @param deleteExtra {@code true} to remove extra files at the destination
         * @return this builder
         */
        public Builder deleteExtra(boolean deleteExtra) { this.deleteExtra = deleteExtra; return this; }

        /**
         * Max concurrent transfer jobs (0 = default).
         *
         * @param jobs maximum number of parallel transfer jobs, or {@code 0} for the default
         * @return this builder
         */
        public Builder jobs(int jobs)                   { this.jobs = jobs; return this; }

        /**
         * Builds and returns the {@link PullOptions}.
         *
         * @return a new {@link PullOptions} with the configured values
         */
        public PullOptions build() { return new PullOptions(this); }
    }
}
