#!/usr/bin/env node
// app.mjs — canonical example: snapdir Node.js binding CLI
//
// Demonstrates the @snapdir/snapdir binding API over a shared S3 store.
// The store URI and credentials are read from the environment:
//   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
//
// CLI:
//   app push <dir> <store>              → prints the 64-hex snapshot id
//   app pull <id>  <store> <dest>       → materialises snapshot into dest
//   app id   <dir>                      → prints the 64-hex snapshot id
//   app diff <store@id_a> <store@id_b>  → prints STATUS<TAB>PATH per line

import * as snapdir from '@snapdir/snapdir'
import { mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const [,, cmd, ...args] = process.argv

// Parse a "store@id" reference into { store, id }.
// The last '@' splits the store URI from the 64-hex snapshot id.
function parseRef(ref) {
  const at = ref.lastIndexOf('@')
  if (at === -1) return { store: ref, id: null }
  return { store: ref.slice(0, at), id: ref.slice(at + 1) }
}

// Format diff entries as porcelain output: STATUS<TAB>PATH per line.
// Matches the snapdir CLI diff output format byte-for-byte.
function porcelain(entries) {
  if (entries.length === 0) return ''
  return entries.map(e => `${e.status}\t${e.path}`).join('\n') + '\n'
}

switch (cmd) {
  case 'push': {
    // push <dir> <store> — stage dir and upload to store; print snapshot id.
    const id = await snapdir.push(args[0], args[1])
    console.log(id)
    break
  }

  case 'pull': {
    // pull <id> <store> <dest> — fetch snapshot from store and materialise.
    await snapdir.pull(args[0], args[1], args[2])
    break
  }

  case 'id': {
    // id <dir> — compute and print the snapshot id for dir.
    const id = await snapdir.id(args[0])
    console.log(id)
    break
  }

  case 'diff': {
    // diff <store@id_a> <store@id_b> — compare two pinned snapshots.
    //
    // The binding's diff() compares two STORE contents. To diff two pinned
    // snapshots from the same store we pull each into a temporary directory,
    // push each to its own temporary file store, then diff those two stores.
    const from = parseRef(args[0])
    const to   = parseRef(args[1])

    const tmpFrom = join(tmpdir(), 'sd-diff-from')
    const tmpTo   = join(tmpdir(), 'sd-diff-to')
    const storeFrom = 'file:///tmp/sd-store-from'
    const storeTo   = 'file:///tmp/sd-store-to'

    mkdirSync(tmpFrom, { recursive: true })
    mkdirSync(tmpTo,   { recursive: true })

    await snapdir.pull(from.id, from.store, tmpFrom)
    await snapdir.push(tmpFrom, storeFrom)
    await snapdir.pull(to.id, to.store, tmpTo)
    await snapdir.push(tmpTo, storeTo)

    const entries = await snapdir.diff({ from: [storeFrom], to: [storeTo] })
    process.stdout.write(porcelain(entries))

    try { rmSync(tmpFrom, { recursive: true, force: true }) } catch {}
    try { rmSync(tmpTo,   { recursive: true, force: true }) } catch {}
    break
  }

  default:
    process.stderr.write(`usage: app {push|pull|id|diff} [args...]\n`)
    process.exit(1)
}
