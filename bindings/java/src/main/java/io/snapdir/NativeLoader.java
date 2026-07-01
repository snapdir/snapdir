package io.snapdir;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * Locates and loads the native {@code libsnapdir_ffi} shared library.
 *
 * <p>At class-load time the loader extracts the platform-appropriate cdylib
 * from the JAR resource path {@code resources/native/<os>-<arch>/libsnapdir_ffi.<ext>}
 * to a temporary file, then calls {@link System#load} with the absolute path.
 *
 * <p>Supported platform keys:
 * <ul>
 *   <li>{@code linux-aarch64} - Linux arm64</li>
 *   <li>{@code linux-x86_64}  - Linux x86-64</li>
 *   <li>{@code mac-aarch64}   - macOS Apple Silicon</li>
 *   <li>{@code mac-x86_64}    - macOS Intel</li>
 *   <li>{@code win-x86_64}    - Windows x64</li>
 * </ul>
 *
 * <p>If the resource is missing (e.g. the JAR was assembled without the native
 * artefact for this platform) a clear {@link SnapdirException} is thrown so the
 * caller receives a meaningful diagnostic rather than a bare {@link NullPointerException}.
 */
public final class NativeLoader {

    /** Library base name without prefix or extension. */
    private static final String LIB_NAME = "snapdir_ffi";

    private NativeLoader() {}

    /**
     * Resolves the absolute path of the native library for the current platform,
     * extracts it from the JAR to a temp file, loads it via {@link System#load},
     * and returns the path of the extracted file.
     *
     * <p>This method is idempotent -- calling it multiple times is safe because
     * the library is only loaded once per JVM process (the JVM deduplicates loads
     * for the same native image).
     *
     * @return absolute path of the extracted temporary file
     * @throws SnapdirException if the native resource is not found for this platform
     * @throws UnsatisfiedLinkError if {@link System#load} fails after extraction
     */
    public static String load() throws SnapdirException {
        String platformDir = platformDir();
        String libExt      = libExtension();
        String resourcePath = "/native/" + platformDir + "/lib" + LIB_NAME + "." + libExt;

        InputStream in = NativeLoader.class.getResourceAsStream(resourcePath);
        if (in == null) {
            throw new SnapdirException(
                "INTERNAL",
                "Native library not found in JAR at " + resourcePath +
                " -- rebuild the JAR with the correct platform artifact for " + platformDir
            );
        }

        try {
            Path tmp = Files.createTempFile("libsnapdir_ffi_", "." + libExt);
            tmp.toFile().deleteOnExit();
            try (InputStream src = in; OutputStream dst = Files.newOutputStream(tmp)) {
                src.transferTo(dst);
            }
            String absolutePath = tmp.toAbsolutePath().toString();
            System.load(absolutePath);
            return absolutePath;
        } catch (IOException e) {
            throw new SnapdirException(
                "INTERNAL",
                "Failed to extract native library to temp file: " + e.getMessage(),
                e
            );
        }
    }

    /**
     * Returns the platform directory name, e.g. {@code "linux-aarch64"}, by
     * mapping {@code os.name} + {@code os.arch} system properties.
     */
    static String platformDir() throws SnapdirException {
        String osName = System.getProperty("os.name", "").toLowerCase();
        String osArch = System.getProperty("os.arch", "").toLowerCase();

        String os;
        if (osName.contains("linux")) {
            os = "linux";
        } else if (osName.contains("mac") || osName.contains("darwin")) {
            os = "mac";
        } else if (osName.contains("windows")) {
            os = "win";
        } else {
            throw new SnapdirException(
                "INTERNAL",
                "Unsupported OS for native library loading: " + osName
            );
        }

        String arch;
        if (osArch.equals("aarch64") || osArch.equals("arm64")) {
            arch = "aarch64";
        } else if (osArch.equals("amd64") || osArch.equals("x86_64")) {
            arch = "x86_64";
        } else {
            throw new SnapdirException(
                "INTERNAL",
                "Unsupported CPU architecture for native library loading: " + osArch
            );
        }

        return os + "-" + arch;
    }

    /**
     * Returns the shared-library file extension for the current OS.
     * {@code "so"} on Linux, {@code "dylib"} on macOS, {@code "dll"} on Windows.
     */
    static String libExtension() throws SnapdirException {
        String osName = System.getProperty("os.name", "").toLowerCase();
        if (osName.contains("linux")) {
            return "so";
        } else if (osName.contains("mac") || osName.contains("darwin")) {
            return "dylib";
        } else if (osName.contains("windows")) {
            return "dll";
        }
        throw new SnapdirException(
            "INTERNAL",
            "Unsupported OS for native library extension: " + osName
        );
    }
}
