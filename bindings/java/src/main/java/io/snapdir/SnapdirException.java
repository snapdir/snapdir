package io.snapdir;

/**
 * Checked exception thrown by Snapdir operations when the C ABI signals failure.
 *
 * <p>{@link #getCode()} returns one of the 8 stable ABI error codes:
 * {@code IO_ERROR}, {@code HASH_MISMATCH}, {@code STORE_ERROR}, {@code IN_FLUX},
 * {@code CATALOG_ERROR}, {@code INVALID_ID}, {@code INVALID_STORE}, {@code CONFLICT},
 * or {@code INTERNAL} for unexpected failures.
 */
public class SnapdirException extends Exception {

    private static final long serialVersionUID = 1L;

    /** Stable ABI error code string (e.g. {@code "IO_ERROR"}). */
    private final String code;

    /**
     * Constructs a SnapdirException with the given stable error code and message.
     *
     * @param code    stable ABI error code (e.g. {@code "IO_ERROR"})
     * @param message human-readable description
     */
    public SnapdirException(String code, String message) {
        super("[" + code + "] " + message);
        this.code = code;
    }

    /**
     * Constructs a SnapdirException wrapping a cause.
     *
     * @param code    stable ABI error code
     * @param message human-readable description
     * @param cause   underlying cause
     */
    public SnapdirException(String code, String message, Throwable cause) {
        super("[" + code + "] " + message, cause);
        this.code = code;
    }

    /**
     * Returns the stable ABI error code (e.g. {@code "HASH_MISMATCH"}).
     * The code string is owned by this exception and valid for its lifetime.
     *
     * @return stable error code string
     */
    public String getCode() {
        return code;
    }
}
