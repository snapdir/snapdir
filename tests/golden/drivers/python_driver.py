#!/usr/bin/env python3
"""tests/golden/drivers/python_driver.py — Python parity driver helper.

Invoked by tests/golden/drivers/python.sh as:
    python_driver.py <subcommand> <args...>

Implements the §1 driver protocol (tests/golden/parity_harness.md) by calling the
built `snapdir` PyO3 binding. stdout is byte-exact; diagnostics go to stderr; exit
0 = success, non-zero = failure. The harness scrubs SNAPDIR_STORE/OBJECTS_STORE/
MANIFEST_CONTEXT and sets LC_ALL=C, SNAPDIR_CACHE_DIR, SNAPDIR_CATALOG_DB_PATH,
SNAPDIR_NO_PROGRESS=true; the binding wraps snapdir-api which honors those — we
inherit the env verbatim.

The snapdir API fns (manifest/id/push/fetch/pull) are async coroutines; we drive
them with asyncio.run(). Store args are `snapdir.StoreUri`; the snapshot-id arg to
fetch/pull is a `snapdir.SnapshotId`. LANE: this file lives under tests/golden/
(adversary lane); it only CONSUMES the binding's public surface.
"""

import asyncio
import sys

import snapdir


def die(msg: str, code: int = 1) -> None:
    sys.stderr.write(f"[python_driver] {msg}\n")
    sys.exit(code)


def parse_path_and_opts(argv):
    """Parse `<path> [--no-follow] [--absolute] [--exclude <RE>]...` like rust.sh,
    returning (path, kwargs) where kwargs maps to the binding's keyword-only
    manifest/id options: no_follow / absolute / exclude.
    """
    path = None
    no_follow = False
    absolute = False
    exclude = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--no-follow":
            no_follow = True
        elif a == "--absolute":
            absolute = True
        elif a == "--exclude":
            i += 1
            if i >= len(argv):
                die("--exclude requires an argument", 2)
            exclude.append(argv[i])
        elif a.startswith("--exclude="):
            exclude.append(a[len("--exclude="):])
        elif a.startswith("-"):
            die(f"unknown flag '{a}'", 2)
        elif path is None:
            path = a
        else:
            die(f"unexpected extra argument '{a}'", 2)
        i += 1
    if path is None:
        die("a <path> argument is required", 2)
    kwargs = {}
    if no_follow:
        kwargs["no_follow"] = True
    if absolute:
        kwargs["absolute"] = True
    if exclude:
        kwargs["exclude"] = exclude
    return path, kwargs


def main() -> None:
    if len(sys.argv) < 2:
        die("usage: python_driver.py {manifest|id|push|fetch|checkout} <args...>", 2)
    sub = sys.argv[1]
    rest = sys.argv[2:]

    if sub == "manifest":
        path, opts = parse_path_and_opts(rest)
        m = asyncio.run(snapdir.manifest(path, **opts))
        # §1.1: emit the raw manifest TEXT byte-exact, INCLUDING the trailing \n.
        # The binding's Manifest.raw is the core Manifest Display; append a single
        # \n iff absent (mirrors the byte contract; BLAKE3 of the rendered bytes
        # equals the binding's own id(), checked by the harness id-self-consistency).
        raw = m.raw if m.raw.endswith("\n") else m.raw + "\n"
        sys.stdout.write(raw)

    elif sub == "id":
        path, opts = parse_path_and_opts(rest)
        sid = asyncio.run(snapdir.id(path, **opts))
        # §1.2: 64-char lowercase hex + a single \n.
        sys.stdout.write(f"{sid}\n")

    elif sub == "push":
        # push <path> <store_uri> [--jobs N]... (tuning args ignored)
        if len(rest) < 2:
            die("push requires <path> <store_uri>", 2)
        path, store_uri = rest[0], rest[1]
        sid = asyncio.run(snapdir.push(path, snapdir.StoreUri(store_uri)))
        sys.stdout.write(f"{sid}\n")

    elif sub == "fetch":
        if len(rest) < 2:
            die("fetch requires <id> <store_uri>", 2)
        sid, store_uri = rest[0], rest[1]
        asyncio.run(snapdir.fetch(snapdir.SnapshotId(sid), snapdir.StoreUri(store_uri)))

    elif sub == "checkout":
        # checkout <id> <store_uri> <dest> → binding pull(id, store, dest)
        if len(rest) < 3:
            die("checkout requires <id> <store_uri> <dest>", 2)
        sid, store_uri, dest = rest[0], rest[1], rest[2]
        asyncio.run(
            snapdir.pull(snapdir.SnapshotId(sid), snapdir.StoreUri(store_uri), dest)
        )

    else:
        die(f"unknown subcommand '{sub}'", 2)


if __name__ == "__main__":
    try:
        main()
    except snapdir.SnapdirError as e:  # type: ignore[attr-defined]
        code = getattr(e, "code", "")
        die(f"{sys.argv[1] if len(sys.argv) > 1 else '?'} failed: "
            f"{code + ': ' if code else ''}{e}")
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001 — driver boundary
        die(f"unexpected error: {e!r}")
