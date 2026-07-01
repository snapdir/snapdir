// tests/golden/drivers/node_driver.mjs — JS half of the Node parity driver.
//
// Invoked by tests/golden/drivers/node.sh as:
//   node node_driver.mjs <subcommand> <args...>
//
// It imports the BUILT @snapdir/snapdir binding (bindings/node/index.mjs) by
// absolute path and calls manifest/id/push/fetch/pull per the §1 driver
// protocol in tests/golden/parity_harness.md. stdout is byte-exact; all
// diagnostics go to stderr; exit 0 = success, non-zero = failure.
//
// LANE NOTE: this file lives under tests/golden/ (adversary lane). It only
// CONSUMES the binding's public surface — it never edits bindings/node/src/.

import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

// Resolve the built binding entry from the workspace root (…/tests/golden/drivers
// → workspace root is three levels up). Import its ESM entry by absolute path so
// we exercise the same module a consumer would `import '@snapdir/snapdir'`.
const WORKSPACE_ROOT = resolve(__dirname, '..', '..', '..')
const BINDING_ENTRY = resolve(WORKSPACE_ROOT, 'bindings', 'node', 'index.mjs')

function die(msg, code = 1) {
  process.stderr.write(`[node_driver] ${msg}\n`)
  process.exit(code)
}

// Parse `<path> [--no-follow] [--absolute] [--exclude <RE>]...` the same way the
// oracle reference driver (rust.sh) does: the path is the single non-flag arg;
// the three manifest flags are collected so we can detect when the binding's
// option-less surface cannot honor them.
function parseManifestArgs(argv) {
  let path = null
  let noFollow = false
  let absolute = false
  const exclude = []
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--no-follow') {
      noFollow = true
    } else if (a === '--absolute') {
      absolute = true
    } else if (a === '--exclude') {
      i++
      if (i >= argv.length) die('--exclude requires an argument', 2)
      exclude.push(argv[i])
    } else if (a.startsWith('--exclude=')) {
      exclude.push(a.slice('--exclude='.length))
    } else if (a.startsWith('-')) {
      die(`unknown flag '${a}'`, 2)
    } else if (path === null) {
      path = a
    } else {
      die(`unexpected extra argument '${a}'`, 2)
    }
  }
  if (path === null) die('a <path> argument is required', 2)
  return { path, noFollow, absolute, exclude }
}

// The binding's manifest(path, options?) / id(path, options?) now accept the
// ManifestOptions surface { noFollow, absolute, exclude } (added in code b534bcc
// → snapdir_api::ManifestOptions). Map the parsed CLI flags to that object.
// Returns `undefined` when no option flag is present so the no-options default
// path is exercised byte-for-byte (omitting === default — backward-compat).
function buildManifestOptions(opts) {
  const o = {}
  if (opts.noFollow) o.noFollow = true
  if (opts.absolute) o.absolute = true
  if (opts.exclude.length) o.exclude = opts.exclude
  return Object.keys(o).length > 0 ? o : undefined
}

async function main() {
  const [, , sub, ...rest] = process.argv
  if (!sub) die('usage: node_driver.mjs {manifest|id|push|fetch|checkout} <args...>', 2)

  let binding
  try {
    binding = await import(BINDING_ENTRY)
  } catch (e) {
    die(`failed to import built binding at ${BINDING_ENTRY}: ${e && e.stack ? e.stack : e}`, 1)
  }

  switch (sub) {
    case 'manifest': {
      const opts = parseManifestArgs(rest)
      const m = await binding.manifest(opts.path, buildManifestOptions(opts))
      // §1.1: emit the raw manifest TEXT byte-exact, INCLUDING the trailing \n.
      // The binding's Manifest.raw is the Display output of the core Manifest.
      // EMPIRICAL: the napi surface exposes `raw` WITHOUT the trailing newline
      // that `snapdir manifest` emits (the core's id is computed over text WITH
      // the newline, so id() is correct, but the exposed `raw` drops it). The
      // §2.1 byte contract REQUIRES exactly one trailing \n, so we render it
      // here: emit `raw` and append a single \n iff it is absent. Use
      // process.stdout.write (NOT console.log, which would unconditionally
      // append a \n → a double-newline byte mismatch). This is driver-side TEXT
      // rendering of the manifest the binding produced, not a content change:
      // the BLAKE3 of the rendered bytes equals the binding's own id() (verified
      // by the harness's id-self-consistency check).
      const raw = m.raw.endsWith('\n') ? m.raw : `${m.raw}\n`
      process.stdout.write(raw)
      break
    }

    case 'id': {
      const opts = parseManifestArgs(rest)
      const sid = await binding.id(opts.path, buildManifestOptions(opts))
      // §1.2: 64-char lowercase hex + a single \n.
      process.stdout.write(`${sid}\n`)
      break
    }

    case 'push': {
      // push <path> <store_uri> [--jobs N]...  (tuning args accepted + ignored)
      const path = rest[0]
      const storeUri = rest[1]
      if (!path || !storeUri) die('push requires <path> <store_uri>', 2)
      const sid = await binding.push(path, storeUri)
      // §1.3: emit the 64-hex id + \n (identical to push stdout contract).
      process.stdout.write(`${sid}\n`)
      break
    }

    case 'fetch': {
      // fetch <id> <store_uri>; stdout unspecified; exit 0 iff retrievable+verifies.
      const id = rest[0]
      const storeUri = rest[1]
      if (!id || !storeUri) die('fetch requires <id> <store_uri>', 2)
      await binding.fetch(id, storeUri)
      break
    }

    case 'checkout': {
      // checkout <id> <store_uri> <dest> → binding pull(id, storeUri, dest);
      // dest must re-manifest to <id> (verified by the harness via the oracle).
      const id = rest[0]
      const storeUri = rest[1]
      const dest = rest[2]
      if (!id || !storeUri || !dest) die('checkout requires <id> <store_uri> <dest>', 2)
      await binding.pull(id, storeUri, dest)
      break
    }

    default:
      die(`unknown subcommand '${sub}'`, 2)
  }
}

main().catch((e) => {
  // SnapdirError or any failure → diagnostic to stderr, non-zero exit.
  const code = e && e.code ? ` [${e.code}]` : ''
  process.stderr.write(`[node_driver] error${code}: ${e && e.stack ? e.stack : e}\n`)
  process.exit(1)
})
