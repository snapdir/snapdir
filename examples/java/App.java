// App.java — canonical example: snapdir Java binding CLI
//
// Demonstrates the snapdir Java binding API over a shared S3 store.
// The store URI and credentials are read from the environment:
//   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
//
// CLI:
//   App push <dir> <store>              → prints the 64-hex snapshot id
//   App pull <id>  <store> <dest>       → materialises snapshot into dest
//   App id   <dir>                      → prints the 64-hex snapshot id
//   App diff <store@id_a> <store@id_b>  → prints STATUS<TAB>PATH per line
//
// Compile: javac --release 17 --add-modules jdk.incubator.foreign -cp snapdir.jar App.java
// Run:     java  --add-modules jdk.incubator.foreign
//               --enable-native-access=ALL-UNNAMED -cp snapdir.jar:. App <args>

import io.snapdir.DiffEntry;
import io.snapdir.DiffStatus;
import io.snapdir.Snapdir;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Comparator;
import java.util.List;

public class App {

    /** Split "store@id" into {store, id}. The last '@' is the delimiter. */
    static String[] parseRef(String ref) {
        int at = ref.lastIndexOf('@');
        if (at == -1) return new String[]{ref, ""};
        return new String[]{ref.substring(0, at), ref.substring(at + 1)};
    }

    /** Map a DiffStatus enum value to its single-character status code. */
    static char statusChar(DiffStatus s) {
        switch (s) {
            case ADDED:     return 'A';
            case DELETED:   return 'D';
            case MODIFIED:  return 'M';
            case UNCHANGED: return '=';
            default:        return '?';
        }
    }

    /** Recursively delete a directory tree (best effort). */
    static void deleteTree(Path root) {
        try {
            Files.walk(root)
                 .sorted(Comparator.reverseOrder())
                 .map(Path::toFile)
                 .forEach(File::delete);
        } catch (IOException ignored) {}
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println("usage: App {push|pull|id|diff} [args...]");
            System.exit(1);
        }

        String cmd = args[0];

        if ("push".equals(cmd)) {
            // push <dir> <store> — stage dir and upload to store; print snapshot id.
            String id = Snapdir.push(args[1], args[2], null).get();
            System.out.println(id);

        } else if ("pull".equals(cmd)) {
            // pull <id> <store> <dest> — fetch snapshot from store and materialise.
            Snapdir.pull(args[1], args[2], args[3], null).get();

        } else if ("id".equals(cmd)) {
            // id <dir> — compute and print the snapshot id for dir.
            String id = Snapdir.id(args[1], null);
            System.out.println(id);

        } else if ("diff".equals(cmd)) {
            // diff <store@id_a> <store@id_b> — compare two pinned snapshots.
            //
            // The binding's diff() compares two STORE contents. To diff two pinned
            // snapshots from the same store we pull each into a temporary directory,
            // push each to its own temporary file store, then diff those two stores.
            String[] fromRef = parseRef(args[1]);
            String[] toRef   = parseRef(args[2]);
            String storeFrom = fromRef[0], idFrom = fromRef[1];
            String storeTo   = toRef[0],   idTo   = toRef[1];

            Path tmp = Files.createTempDirectory("sd-diff-");
            try {
                Path dirFrom    = tmp.resolve("from");
                Path dirTo      = tmp.resolve("to");
                String fstoreFrom = "file://" + tmp.resolve("store-from").toAbsolutePath();
                String fstoreTo   = "file://" + tmp.resolve("store-to").toAbsolutePath();

                Files.createDirectories(dirFrom);
                Files.createDirectories(dirTo);

                Snapdir.pull(idFrom, storeFrom, dirFrom.toAbsolutePath().toString(), null).get();
                Snapdir.push(dirFrom.toAbsolutePath().toString(), fstoreFrom, null).get();
                Snapdir.pull(idTo,   storeTo,   dirTo.toAbsolutePath().toString(), null).get();
                Snapdir.push(dirTo.toAbsolutePath().toString(),   fstoreTo, null).get();

                List<DiffEntry> entries = Snapdir.diff(fstoreFrom, fstoreTo, null).get();

                // Print as STATUS<TAB>PATH per line — matches the snapdir CLI diff format.
                for (DiffEntry e : entries) {
                    System.out.printf("%c\t%s%n", statusChar(e.status()), e.path());
                }
            } finally {
                deleteTree(tmp);
            }

        } else {
            System.err.println("unknown command: " + cmd);
            System.exit(1);
        }
    }
}
