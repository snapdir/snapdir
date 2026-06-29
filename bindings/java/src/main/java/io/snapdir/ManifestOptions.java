package io.snapdir;

/**
 * Options controlling the walk behaviour for {@link Snapdir#manifest} and
 * {@link Snapdir#id}.
 *
 * <p>All fields default to the C ABI {@code NULL}/0/{@code false} defaults.
 * Use the builder to override specific fields without telescoping constructors:
 *
 * <pre>{@code
 * ManifestOptions opts = ManifestOptions.builder()
 *     .noFollow(true)
 *     .exclude("(?i)\\.git")
 *     .build();
 * }</pre>
 */
public final class ManifestOptions {

    /** Extended-regex exclusion pattern; {@code null} = no exclusion. */
    private final String exclude;

    /** Parallel hashing worker count; {@code 0} = auto (CPU-count default). */
    private final int walkJobs;

    /** Emit absolute paths instead of {@code ./}-relative paths. */
    private final boolean absolute;

    /** Do not follow symbolic links during the walk. */
    private final boolean noFollow;

    /**
     * Checksum algorithm override; {@code null} = {@code "b3sum"} default.
     * Pass {@code "md5sum"} or {@code "sha256sum"} to select those algorithms.
     */
    private final String checksumBin;

    /**
     * Local object-cache directory override; {@code null} = default location.
     */
    private final String cacheDir;

    /**
     * Catalog adapter selection; {@code null} = default adapter;
     * {@code "none"} = suppress catalog recording.
     */
    private final String catalog;

    private ManifestOptions(Builder b) {
        this.exclude     = b.exclude;
        this.walkJobs    = b.walkJobs;
        this.absolute    = b.absolute;
        this.noFollow    = b.noFollow;
        this.checksumBin = b.checksumBin;
        this.cacheDir    = b.cacheDir;
        this.catalog     = b.catalog;
    }

    /**
     * Returns the exclusion regex pattern, or {@code null}.
     *
     * @return extended-regex exclusion pattern, or {@code null} for no exclusion
     */
    public String exclude()     { return exclude; }

    /**
     * Returns the parallel walk-job count (0 = auto).
     *
     * @return number of parallel hashing workers, or {@code 0} for the CPU-count default
     */
    public int walkJobs()       { return walkJobs; }

    /**
     * Returns {@code true} if absolute paths should be emitted.
     *
     * @return {@code true} when manifest paths are absolute rather than {@code ./}-relative
     */
    public boolean absolute()   { return absolute; }

    /**
     * Returns {@code true} if symbolic links should not be followed.
     *
     * @return {@code true} when the walk omits symlink targets
     */
    public boolean noFollow()   { return noFollow; }

    /**
     * Returns the checksum algorithm override, or {@code null}.
     *
     * @return checksum algorithm name (e.g. {@code "md5sum"}), or {@code null} for the {@code "b3sum"} default
     */
    public String checksumBin() { return checksumBin; }

    /**
     * Returns the cache directory override, or {@code null}.
     *
     * @return local object-cache directory path, or {@code null} for the default location
     */
    public String cacheDir()    { return cacheDir; }

    /**
     * Returns the catalog adapter selection, or {@code null}.
     *
     * @return catalog adapter name, {@code "none"} to suppress recording, or {@code null} for the default
     */
    public String catalog()     { return catalog; }

    /**
     * Returns a new builder with all defaults.
     *
     * @return a fresh {@link Builder}
     */
    public static Builder builder() { return new Builder(); }

    /** Builder for {@link ManifestOptions}. */
    public static final class Builder {
        private String  exclude;
        private int     walkJobs;
        private boolean absolute;
        private boolean noFollow;
        private String  checksumBin;
        private String  cacheDir;
        private String  catalog;

        private Builder() {}

        /**
         * Extended-regex exclusion pattern; {@code null} = no exclusion.
         *
         * @param exclude POSIX extended-regex pattern matched against each path, or {@code null}
         * @return this builder
         */
        public Builder exclude(String exclude)         { this.exclude = exclude; return this; }

        /**
         * Parallel hashing worker count; {@code 0} = auto.
         *
         * @param walkJobs number of parallel hashing workers, or {@code 0} for the CPU-count default
         * @return this builder
         */
        public Builder walkJobs(int walkJobs)          { this.walkJobs = walkJobs; return this; }

        /**
         * Emit absolute paths instead of {@code ./}-relative paths.
         *
         * @param absolute {@code true} to emit absolute paths in the manifest
         * @return this builder
         */
        public Builder absolute(boolean absolute)      { this.absolute = absolute; return this; }

        /**
         * Do not follow symbolic links during the walk.
         *
         * @param noFollow {@code true} to omit symlink targets from the manifest
         * @return this builder
         */
        public Builder noFollow(boolean noFollow)      { this.noFollow = noFollow; return this; }

        /**
         * Checksum algorithm override; {@code null} = {@code "b3sum"}.
         *
         * @param checksumBin algorithm name (e.g. {@code "md5sum"} or {@code "sha256sum"}), or {@code null}
         * @return this builder
         */
        public Builder checksumBin(String checksumBin) { this.checksumBin = checksumBin; return this; }

        /**
         * Local object-cache directory override.
         *
         * @param cacheDir path to the local cache directory, or {@code null} for the default
         * @return this builder
         */
        public Builder cacheDir(String cacheDir)       { this.cacheDir = cacheDir; return this; }

        /**
         * Catalog adapter selection.
         *
         * @param catalog adapter name, or {@code "none"} to suppress catalog recording
         * @return this builder
         */
        public Builder catalog(String catalog)         { this.catalog = catalog; return this; }

        /**
         * Builds and returns the {@link ManifestOptions}.
         *
         * @return a new {@link ManifestOptions} with the configured values
         */
        public ManifestOptions build() { return new ManifestOptions(this); }
    }
}
