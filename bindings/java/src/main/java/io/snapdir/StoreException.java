package io.snapdir;

/**
 * Thrown when a STORE_ERROR is returned by the C ABI.
 *
 * <p>Indicates a failure communicating with or accessing the backing object store
 * (e.g. a file:// path that is not writable, or an S3/GCS/B2 transport error).
 */
public class StoreException extends SnapdirException {

    private static final long serialVersionUID = 1L;

    /**
     * Constructs a StoreException with the given detail message.
     *
     * @param message human-readable description of the store failure
     */
    public StoreException(String message) {
        super("STORE_ERROR", message);
    }
}
