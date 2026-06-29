package io.snapdir;

/**
 * Thrown when a CATALOG_ERROR is returned by the C ABI.
 *
 * <p>Indicates a failure reading or writing to the local snapshot catalog
 * (SQLite database). The catalog records location history for push operations.
 */
public class CatalogException extends SnapdirException {

    private static final long serialVersionUID = 1L;

    /**
     * Constructs a CatalogException with the given detail message.
     *
     * @param message human-readable description of the catalog failure
     */
    public CatalogException(String message) {
        super("CATALOG_ERROR", message);
    }
}
