// Command parity-driver is the Go-binding driver for the cross-language parity
// harness (tests/golden/run_parity.sh, §1 protocol). It exercises the public
// `snapdir` Go binding (CGo over the C ABI) and emits BYTE-EXACT stdout:
//
//	parity-driver manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
//	parity-driver id       <path> [--no-follow] [--absolute] [--exclude <RE>]...
//	parity-driver push     <path> <store_uri> [--jobs N]
//	parity-driver fetch    <id>   <store_uri>
//	parity-driver checkout <id>   <store_uri> <dest>
//
// stdout is byte-exact per the spec; diagnostics go to stderr; exit 0 = success.
// The harness scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT and sets
// LC_ALL=C, SNAPDIR_CACHE_DIR, SNAPDIR_CATALOG_DB_PATH, SNAPDIR_NO_PROGRESS; the
// binding (via the C ABI → snapdir-api) honors those — we inherit the env.
package main

import (
	"context"
	"fmt"
	"os"
	"strings"

	snapdir "github.com/snapdir/snapdir/bindings/go"
)

func die(format string, a ...any) {
	fmt.Fprintf(os.Stderr, "[parity-driver] "+format+"\n", a...)
	os.Exit(1)
}

// parsePathAndOpts parses `<path> [--no-follow] [--absolute] [--exclude <RE>]...`
// into the path and the native *snapdir.ManifestOptions (the Go binding gets
// these options natively from the C ABI, so symlinks-nofollow etc. are reachable).
func parsePathAndOpts(args []string) (string, *snapdir.ManifestOptions) {
	path := ""
	opts := &snapdir.ManifestOptions{}
	any := false
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--no-follow":
			opts.NoFollow = true
			any = true
		case a == "--absolute":
			opts.Absolute = true
			any = true
		case a == "--exclude":
			i++
			if i >= len(args) {
				die("--exclude requires an argument")
			}
			opts.Exclude = append(opts.Exclude, args[i])
			any = true
		case strings.HasPrefix(a, "--exclude="):
			opts.Exclude = append(opts.Exclude, a[len("--exclude="):])
			any = true
		case strings.HasPrefix(a, "-"):
			die("unknown flag %q", a)
		case path == "":
			path = a
		default:
			die("unexpected extra argument %q", a)
		}
	}
	if path == "" {
		die("a <path> argument is required")
	}
	if !any {
		return path, nil // nil opts == defaults
	}
	return path, opts
}

func main() {
	if len(os.Args) < 2 {
		die("usage: parity-driver {manifest|id|push|fetch|checkout} <args...>")
	}
	ctx := context.Background()
	sub := os.Args[1]
	rest := os.Args[2:]

	switch sub {
	case "manifest":
		path, opts := parsePathAndOpts(rest)
		m, err := snapdir.Manifest(ctx, path, opts)
		if err != nil {
			die("manifest failed: %v", err)
		}
		// §1.1: emit the raw manifest TEXT byte-exact, including the trailing \n.
		raw := m.Raw
		if !strings.HasSuffix(raw, "\n") {
			raw += "\n"
		}
		os.Stdout.WriteString(raw)

	case "id":
		path, opts := parsePathAndOpts(rest)
		id, err := snapdir.ID(ctx, path, opts)
		if err != nil {
			die("id failed: %v", err)
		}
		fmt.Println(id) // 64-hex + \n

	case "push":
		// push <path> <store_uri> [--jobs N]... (tuning args ignored)
		if len(rest) < 2 {
			die("push requires <path> <store_uri>")
		}
		id, err := snapdir.Push(ctx, rest[0], rest[1])
		if err != nil {
			die("push failed: %v", err)
		}
		fmt.Println(id)

	case "fetch":
		if len(rest) < 2 {
			die("fetch requires <id> <store_uri>")
		}
		if err := snapdir.Fetch(ctx, rest[0], rest[1]); err != nil {
			die("fetch failed: %v", err)
		}

	case "checkout":
		// checkout <id> <store_uri> <dest> → Pull(id, store, dest)
		if len(rest) < 3 {
			die("checkout requires <id> <store_uri> <dest>")
		}
		if err := snapdir.Pull(ctx, rest[0], rest[1], rest[2]); err != nil {
			die("checkout failed: %v", err)
		}

	default:
		die("unknown subcommand %q", sub)
	}
}
