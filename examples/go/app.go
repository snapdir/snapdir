// app.go — canonical example: snapdir Go binding CLI
//
// Demonstrates the snapdir Go CGo binding API over a shared S3 store.
// The store URI and credentials are read from the environment:
//   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
//
// CLI:
//   app push <dir> <store>              → prints the 64-hex snapshot id
//   app pull <id>  <store> <dest>       → materialises snapshot into dest
//   app id   <dir>                      → prints the 64-hex snapshot id
//   app diff <store@id_a> <store@id_b>  → prints STATUS<TAB>PATH per line
package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	snapdir "github.com/snapdir/snapdir/bindings/go"
)

// parseRef splits a "store@id" reference into (store, id).
// The last '@' is the delimiter; a URI with no '@' returns (uri, "").
func parseRef(ref string) (store, id string) {
	at := strings.LastIndex(ref, "@")
	if at == -1 {
		return ref, ""
	}
	return ref[:at], ref[at+1:]
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: app {push|pull|id|diff} [args...]")
		os.Exit(1)
	}

	cmd, args := os.Args[1], os.Args[2:]
	ctx := context.Background()

	switch cmd {
	case "push":
		// push <dir> <store> — stage dir and upload to store; print snapshot id.
		id, err := snapdir.Push(ctx, args[0], args[1])
		if err != nil {
			fmt.Fprintln(os.Stderr, "push:", err)
			os.Exit(1)
		}
		fmt.Println(id)

	case "pull":
		// pull <id> <store> <dest> — fetch snapshot from store and materialise.
		if err := snapdir.Pull(ctx, args[0], args[1], args[2]); err != nil {
			fmt.Fprintln(os.Stderr, "pull:", err)
			os.Exit(1)
		}

	case "id":
		// id <dir> — compute and print the snapshot id for dir.
		id, err := snapdir.ID(ctx, args[0], nil)
		if err != nil {
			fmt.Fprintln(os.Stderr, "id:", err)
			os.Exit(1)
		}
		fmt.Println(id)

	case "diff":
		// diff <store@id_a> <store@id_b> — compare two pinned snapshots.
		//
		// The binding's Diff() compares two STORE contents. To diff two pinned
		// snapshots from the same store we pull each into a temporary directory,
		// push each to its own temporary file store, then diff those two stores.
		storeFrom, idFrom := parseRef(args[0])
		storeTo, idTo := parseRef(args[1])

		tmpDir, err := os.MkdirTemp("", "sd-diff-*")
		if err != nil {
			fmt.Fprintln(os.Stderr, "mkdtemp:", err)
			os.Exit(1)
		}
		defer os.RemoveAll(tmpDir)

		dirFrom := filepath.Join(tmpDir, "from")
		dirTo := filepath.Join(tmpDir, "to")
		fstoreFrom := "file://" + filepath.Join(tmpDir, "store-from")
		fstoreTo := "file://" + filepath.Join(tmpDir, "store-to")

		if err := snapdir.Pull(ctx, idFrom, storeFrom, dirFrom); err != nil {
			fmt.Fprintln(os.Stderr, "pull from:", err)
			os.Exit(1)
		}
		if _, err := snapdir.Push(ctx, dirFrom, fstoreFrom); err != nil {
			fmt.Fprintln(os.Stderr, "push from:", err)
			os.Exit(1)
		}
		if err := snapdir.Pull(ctx, idTo, storeTo, dirTo); err != nil {
			fmt.Fprintln(os.Stderr, "pull to:", err)
			os.Exit(1)
		}
		if _, err := snapdir.Push(ctx, dirTo, fstoreTo); err != nil {
			fmt.Fprintln(os.Stderr, "push to:", err)
			os.Exit(1)
		}

		entries, err := snapdir.Diff(ctx, fstoreFrom, fstoreTo)
		if err != nil {
			fmt.Fprintln(os.Stderr, "diff:", err)
			os.Exit(1)
		}
		// Print as STATUS<TAB>PATH per line — matches the snapdir CLI diff format.
		for _, e := range entries {
			fmt.Printf("%c\t%s\n", byte(e.Status), e.Path)
		}

	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n", cmd)
		os.Exit(1)
	}
}
