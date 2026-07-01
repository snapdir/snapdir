package io.snapdir;

/**
 * Thrown when a HASH_MISMATCH error is returned by the C ABI.
 *
 * <p>Indicates that a fetched or verified object's BLAKE3 hash does not match
 * its expected value. The snapshot may be corrupt or the store may be inconsistent.
 */
public class HashMismatchException extends SnapdirException {

    private static final long serialVersionUID = 1L;

    /**
     * Constructs a HashMismatchException with the given detail message.
     *
     * @param message human-readable description of the hash mismatch
     */
    public HashMismatchException(String message) {
        super("HASH_MISMATCH", message);
    }
}
