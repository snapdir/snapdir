package io.snapdir;

/**
 * Options controlling the behaviour of {@link Snapdir#push}.
 *
 * <p>All fields default to the C ABI {@code NULL}/0 defaults.
 */
public final class PushOptions {

    /**
     * 64-hex id of a previously-staged snapshot; {@code null} = use the source
     * path (the common case).
     */
    private final String sourceId;

    /** Max concurrent transfer jobs; {@code 0} = default. */
    private final int jobs;

    /**
     * Bandwidth cap string (e.g. {@code "10M"}); {@code null} = unlimited.
     */
    private final String limitRate;

    /** Max retry attempts per object; {@code 0} = default (5). */
    private final int maxRetries;

    /** Local cache directory override; {@code null} = default. */
    private final String cacheDir;

    private PushOptions(Builder b) {
        this.sourceId   = b.sourceId;
        this.jobs       = b.jobs;
        this.limitRate  = b.limitRate;
        this.maxRetries = b.maxRetries;
        this.cacheDir   = b.cacheDir;
    }

    /**
     * Returns the pre-staged snapshot id, or {@code null}.
     *
     * @return 64-hex snapshot id of a previously-staged snapshot, or {@code null} to use the source path
     */
    public String sourceId()   { return sourceId; }

    /**
     * Returns the max concurrent transfer jobs (0 = default).
     *
     * @return maximum number of parallel transfer jobs, or {@code 0} for the default
     */
    public int jobs()          { return jobs; }

    /**
     * Returns the bandwidth cap string, or {@code null}.
     *
     * @return bandwidth cap (e.g. {@code "10M"}), or {@code null} for unlimited
     */
    public String limitRate()  { return limitRate; }

    /**
     * Returns the max retry count (0 = default of 5).
     *
     * @return maximum retry attempts per object, or {@code 0} for the default of 5
     */
    public int maxRetries()    { return maxRetries; }

    /**
     * Returns the cache directory override, or {@code null}.
     *
     * @return local cache directory path, or {@code null} for the default
     */
    public String cacheDir()   { return cacheDir; }

    /**
     * Returns a new builder with all defaults.
     *
     * @return a fresh {@link Builder}
     */
    public static Builder builder() { return new Builder(); }

    /** Builder for {@link PushOptions}. */
    public static final class Builder {
        private String  sourceId;
        private int     jobs;
        private String  limitRate;
        private int     maxRetries;
        private String  cacheDir;

        private Builder() {}

        /**
         * Pre-staged snapshot id; {@code null} = use source path.
         *
         * @param sourceId 64-hex id of a previously-staged snapshot, or {@code null}
         * @return this builder
         */
        public Builder sourceId(String sourceId)   { this.sourceId = sourceId; return this; }

        /**
         * Max concurrent transfer jobs (0 = default).
         *
         * @param jobs maximum number of parallel transfer jobs, or {@code 0} for the default
         * @return this builder
         */
        public Builder jobs(int jobs)              { this.jobs = jobs; return this; }

        /**
         * Bandwidth cap (e.g. {@code "10M"}); {@code null} = unlimited.
         *
         * @param limitRate bandwidth cap string (e.g. {@code "10M"} for 10 MB/s), or {@code null}
         * @return this builder
         */
        public Builder limitRate(String limitRate) { this.limitRate = limitRate; return this; }

        /**
         * Max retries per object (0 = default of 5).
         *
         * @param maxRetries maximum retry attempts per object, or {@code 0} for the default of 5
         * @return this builder
         */
        public Builder maxRetries(int maxRetries)  { this.maxRetries = maxRetries; return this; }

        /**
         * Local cache directory override.
         *
         * @param cacheDir path to the local cache directory, or {@code null} for the default
         * @return this builder
         */
        public Builder cacheDir(String cacheDir)   { this.cacheDir = cacheDir; return this; }

        /**
         * Builds and returns the {@link PushOptions}.
         *
         * @return a new {@link PushOptions} with the configured values
         */
        public PushOptions build() { return new PushOptions(this); }
    }
}
