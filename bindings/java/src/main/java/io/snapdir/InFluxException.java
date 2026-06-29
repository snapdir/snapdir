package io.snapdir;

/**
 * Thrown when an IN_FLUX error is returned by the C ABI.
 *
 * <p>Indicates that the directory being snapshotted was modified during the walk
 * and the resulting manifest would be inconsistent. The caller should retry.
 */
public class InFluxException extends SnapdirException {

    private static final long serialVersionUID = 1L;

    /**
     * Constructs an InFluxException with the given detail message.
     *
     * @param message human-readable description of the in-flux condition
     */
    public InFluxException(String message) {
        super("IN_FLUX", message);
    }
}
