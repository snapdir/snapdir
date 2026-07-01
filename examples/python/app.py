#!/usr/bin/env python3
# app.py — canonical example: snapdir Python binding CLI
#
# Demonstrates the snapdir Python binding API over a shared S3 store.
# The store URI and credentials are read from the environment:
#   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
#
# CLI:
#   app.py push <dir> <store>              → prints the 64-hex snapshot id
#   app.py pull <id>  <store> <dest>       → materialises snapshot into dest
#   app.py id   <dir>                      → prints the 64-hex snapshot id
#   app.py diff <store@id_a> <store@id_b>  → prints STATUS<TAB>PATH per line
"""snapdir Python binding — minimal relay CLI example."""

from __future__ import annotations

import asyncio
import shutil
import sys
import tempfile

import snapdir
from snapdir import DiffOptions, SnapshotId, StoreUri


def parse_ref(ref: str) -> tuple[str, str | None]:
    """Parse 'store@id' into (store, id).  The last '@' is the delimiter."""
    at = ref.rfind('@')
    if at == -1:
        return ref, None
    return ref[:at], ref[at + 1:]


def porcelain(entries: list[snapdir.DiffEntry]) -> str:
    """Format diff entries as STATUS<TAB>PATH lines (matches snapdir CLI output)."""
    return ''.join(f'{e.status}\t{e.path}\n' for e in entries)


async def main() -> None:
    if len(sys.argv) < 2:
        print('usage: app.py {push|pull|id|diff} [args...]', file=sys.stderr)
        sys.exit(1)

    cmd, *args = sys.argv[1:]

    if cmd == 'push':
        # push <dir> <store> — stage dir and upload to store; print snapshot id.
        snap_id = await snapdir.push(args[0], StoreUri(args[1]))
        print(snap_id)

    elif cmd == 'pull':
        # pull <id> <store> <dest> — fetch snapshot from store and materialise.
        await snapdir.pull(SnapshotId(args[0]), StoreUri(args[1]), args[2])

    elif cmd == 'id':
        # id <dir> — compute and print the snapshot id for dir.
        snap_id = await snapdir.id(args[0])
        print(snap_id)

    elif cmd == 'diff':
        # diff <store@id_a> <store@id_b> — compare two pinned snapshots.
        #
        # The binding's diff() compares two STORE contents. To diff two pinned
        # snapshots from the same store we pull each into a temporary directory,
        # push each to its own temporary file store, then diff those two stores.
        store_from, id_from = parse_ref(args[0])
        store_to,   id_to   = parse_ref(args[1])

        with tempfile.TemporaryDirectory(prefix='sd-diff-') as tmp:
            dir_from  = f'{tmp}/from'
            dir_to    = f'{tmp}/to'
            fstore_from = f'file://{tmp}/store-from'
            fstore_to   = f'file://{tmp}/store-to'

            await snapdir.pull(SnapshotId(id_from), StoreUri(store_from), dir_from)
            await snapdir.push(dir_from, StoreUri(fstore_from))
            await snapdir.pull(SnapshotId(id_to),   StoreUri(store_to),   dir_to)
            await snapdir.push(dir_to,   StoreUri(fstore_to))

            entries = await snapdir.diff(DiffOptions.from_refs([fstore_from], [fstore_to]))
            sys.stdout.write(porcelain(entries))

    else:
        print(f'unknown command: {cmd}', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    asyncio.run(main())
