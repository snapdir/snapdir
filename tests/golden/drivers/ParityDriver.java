// tests/golden/drivers/ParityDriver.java  Java parity driver helper.
//
// Invoked by tests/golden/drivers/java.sh as:
//     java ... io.snapdir.ParityDriver <subcommand> <args...>
//
// Implements the 1 driver protocol (tests/golden/parity_harness.md) by calling
// the built `io.snapdir` JDK-Foreign binding. stdout is byte-exact; diagnostics
// go to stderr; exit 0 = success, non-zero = failure. The harness scrubs
// SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT and sets LC_ALL=C, SNAPDIR_CACHE_DIR,
// SNAPDIR_CATALOG_DB_PATH, SNAPDIR_NO_PROGRESS=1; the binding wraps snapdir-api (via
// the C ABI) which honors those  we inherit the env verbatim.
//
// LANE NOTE: this file lives under tests/golden/  it only CONSUMES the binding's
// public surface (io.snapdir.Snapdir + option builders); it never reimplements the
// binding and is NOT part of the published jar (compiled into build/classes only).
// Mirrors python_driver.py / the prebuilt go/zig/cpp drivers' exec-the-binding shape.
package io.snapdir;

import java.io.PrintStream;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletionException;

public final class ParityDriver {

    private static void die(String msg, int code) {
        System.err.println("[java_driver] " + msg);
        System.exit(code);
    }

    /** Parse `<path> [--no-follow] [--absolute] [--exclude <RE>]...` like rust.sh. */
    private static ManifestOptions parsePathAndOpts(String[] argv, String[] outPath) {
        String path = null;
        boolean noFollow = false;
        boolean absolute = false;
        List<String> excludes = new ArrayList<>();
        for (int i = 0; i < argv.length; i++) {
            String a = argv[i];
            switch (a) {
                case "--no-follow" -> noFollow = true;
                case "--absolute" -> absolute = true;
                case "--exclude" -> {
                    if (++i >= argv.length) { die("--exclude requires an argument", 2); }
                    excludes.add(argv[i]);
                }
                default -> {
                    if (a.startsWith("--exclude=")) {
                        excludes.add(a.substring("--exclude=".length()));
                    } else if (a.startsWith("-")) {
                        die("unknown flag '" + a + "'", 2);
                    } else if (path == null) {
                        path = a;
                    } else {
                        die("unexpected extra argument '" + a + "'", 2);
                    }
                }
            }
        }
        if (path == null) { die("a <path> argument is required", 2); }
        outPath[0] = path;
        ManifestOptions.Builder b = ManifestOptions.builder()
            .noFollow(noFollow)
            .absolute(absolute);
        // The binding exposes a single exclude regex; the parity fixtures use at
        // most one --exclude (only --no-follow is exercised today). If more than
        // one is ever passed, fail loudly rather than silently dropping any.
        if (excludes.size() == 1) {
            b.exclude(excludes.get(0));
        } else if (excludes.size() > 1) {
            die("the Java binding accepts a single --exclude regex (got "
                + excludes.size() + ")", 2);
        }
        return b.build();
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            die("usage: ParityDriver {manifest|id|push|fetch|checkout} <args...>", 2);
        }
        String sub = args[0];
        String[] rest = new String[args.length - 1];
        System.arraycopy(args, 1, rest, 0, rest.length);
        // A confined writer that never injects extra bytes (no autoflush banners).
        PrintStream out = new PrintStream(System.out, false, "UTF-8");

        try {
            switch (sub) {
                case "manifest" -> {
                    String[] p = new String[1];
                    ManifestOptions opts = parsePathAndOpts(rest, p);
                    Manifest m = Snapdir.manifest(p[0], opts);
                    // 1.1: emit the raw manifest TEXT byte-exact, INCLUDING the
                    // single trailing \n (append iff absent  id == BLAKE3(text)).
                    String raw = m.raw();
                    out.print(raw.endsWith("\n") ? raw : raw + "\n");
                }
                case "id" -> {
                    String[] p = new String[1];
                    ManifestOptions opts = parsePathAndOpts(rest, p);
                    out.print(Snapdir.id(p[0], opts) + "\n");  // 1.2: 64-hex + \n
                }
                case "push" -> {
                    if (rest.length < 2) { die("push requires <path> <store_uri>", 2); }
                    // trailing tuning args (--jobs N, ...) accepted and ignored
                    String id = Snapdir.push(rest[0], rest[1], null).join();
                    out.print(id + "\n");
                }
                case "fetch" -> {
                    if (rest.length < 2) { die("fetch requires <id> <store_uri>", 2); }
                    Snapdir.fetch(rest[0], rest[1], 0).join();
                }
                case "checkout" -> {
                    if (rest.length < 3) {
                        die("checkout requires <id> <store_uri> <dest>", 2);
                    }
                    // 1.5: pull(id, store, dest)  dest re-manifests to id
                    Snapdir.pull(rest[0], rest[1], rest[2], null).join();
                }
                default -> die("unknown subcommand '" + sub + "'", 2);
            }
            out.flush();
        } catch (SnapdirException e) {
            out.flush();
            String code = e.getCode();
            die(sub + " failed: " + (code != null && !code.isEmpty() ? code + ": " : "")
                + e.getMessage(), 1);
        } catch (CompletionException e) {
            out.flush();
            Throwable cause = e.getCause();
            if (cause instanceof SnapdirException se) {
                String code = se.getCode();
                die(sub + " failed: " + (code != null && !code.isEmpty() ? code + ": " : "")
                    + se.getMessage(), 1);
            }
            die(sub + " failed: " + (cause != null ? cause : e), 1);
        }
    }

    private ParityDriver() { }
}
