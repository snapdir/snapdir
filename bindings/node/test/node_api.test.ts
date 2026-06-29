/**
 * node_api.ts — BLACK-BOX spec for the @snapdir/snapdir Node binding (Phase 37,
 * gate `node-api-spec-tests`, adversary/opus).
 *
 * ============================================================================
 * WHAT THIS PINS
 * ----------------------------------------------------------------------------
 * The idiomatic TypeScript contract every Node USER sees over the frozen
 * `snapdir-api` §6 surface (docs/rust-port/PUBLIC_API.md + the M0 review). This
 * is authored from the snapdir-api DESIGN + Node-binding idioms (.gatesmith/
 * templates/node.md) ONLY — the napi impl does not influence it.
 *
 * The HEADLINE Node-idiom requirements pinned here:
 *   1. `ManifestEntry.size` is `bigint` (u64 overflows Number.MAX_SAFE_INTEGER).
 *   2. `SnapdirError instanceof Error` with a non-empty `.code: string` that is
 *      one of the 8 frozen stable codes.
 *   3. I/O / network fns return `Promise<…>` and never block the event loop;
 *      the suite is `--detect-open-handles`-clean (no leaked tokio handles).
 *   4. `idFromManifest` is SYNC (pure hash) and self-consistent with `id()`.
 *   5. ESM (`import … from '@snapdir/snapdir'`) AND CJS (`require(...)`) both
 *      expose the same surface.
 *   6. The snapshot-id type is a 64-lowercase-hex string.
 *   7. A behavioral round-trip (manifest → id → push) — basic shape.
 *
 * ----------------------------------------------------------------------------
 * SYNC / ASYNC MAPPING (explicit — the contract the -impl must satisfy)
 * ----------------------------------------------------------------------------
 * snapdir-api §6 classifies fns Sync vs Async at the *library* level. The Node
 * binding (per .gatesmith/templates/node.md) re-projects the CPU-bound walks as
 * `async` so they run on a thread pool and never block the libuv event loop,
 * while keeping the pure/cheap ones SYNC:
 *
 *   ASYNC (return Promise) in the Node binding:
 *     manifest, id, stage           (snapdir-api SYNC, but CPU-bound walk →
 *                                     exposed async via spawn_blocking so the
 *                                     event loop stays responsive — node.md:
 *                                     "manifest and id are exposed async")
 *     push, fetch, pull, checkout,
 *     sync, diff, verify            (snapdir-api ASYNC, network/I-O-bound)
 *
 *   SYNC (return a value directly) in the Node binding:
 *     idFromManifest                (pure hash, no I/O — node.md: "idFromManifest
 *                                     is sync (pure hash)")
 *     version                       (infallible string)
 *
 *   The local-cache / catalog reads (verifyCache, flushCache, locations,
 *   ancestors, revisions, defaults) are snapdir-api SYNC; this spec leaves their
 *   sync/async projection to the impl/judge and does not over-pin them — the
 *   load-bearing Node-idiom claims above are what this gate exists to nail.
 *
 * ----------------------------------------------------------------------------
 * EXPECTED-FAIL RATIONALE (this is the no-impl state — correct & intended)
 * ----------------------------------------------------------------------------
 * The scaffold's auto-generated `index.d.ts` currently exports ONLY
 * `version(): string`. None of the types (ManifestEntry, SnapdirError,
 * SnapshotId, …) or fns (manifest/id/idFromManifest/push/…) exist yet. So:
 *   • `tsc --strict` will ERROR on every import below that isn't `version`.
 *   • vitest runtime cases will throw/reject (the fns are undefined).
 * That FAILURE *is* the no-impl signal. This file is well-formed TS that WOULD
 * typecheck + pass once the impl lands the full surface + types. The `-impl`
 * gate `git mv`s this into bindings/node/test/ and makes `tsc --strict` +
 * vitest green; `-tests-review` strengthens it.
 *
 * ----------------------------------------------------------------------------
 * BLACK-BOX ATTESTATION
 * ----------------------------------------------------------------------------
 * Authored from the snapdir-api §6 frozen surface + Node idioms ONLY. I did NOT
 * read bindings/node/src/lib.rs (beyond knowing the scaffold exports only
 * `version()`) and reference ZERO napi/Rust internals.
 * ============================================================================
 */

import { describe, it, expect, expectTypeOf, beforeAll, afterAll } from 'vitest'
import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { createRequire } from 'node:module'

// The binding under test (ESM import). Until the impl lands the surface, every
// named import other than `version` is a tsc error — the intended no-impl state.
import {
  version,
  manifest,
  id,
  idFromManifest,
  stage,
  push,
  fetch as snapdirFetch,
  pull,
  checkout,
  sync,
  diff,
  verify,
  SnapdirError,
  // Types (pinned at type-level below). These names are the TS contract the
  // impl's generated .d.ts must expose.
  type Manifest,
  type ManifestEntry,
  type SnapshotId,
  type DiffEntry,
  type DiffStatus,
  type PathType,
} from '@snapdir/snapdir'

// ----------------------------------------------------------------------------
// The 8 FROZEN cross-language error codes (PUBLIC_API.md §4.1). The Node
// SnapdirError.code MUST be exactly one of these strings.
// ----------------------------------------------------------------------------
const STABLE_CODES = [
  'IO_ERROR',
  'HASH_MISMATCH',
  'STORE_ERROR',
  'IN_FLUX',
  'CATALOG_ERROR',
  'INVALID_ID',
  'INVALID_STORE',
  'CONFLICT',
] as const
type StableCode = (typeof STABLE_CODES)[number]

const HEX64 = /^[0-9a-f]{64}$/ // SnapshotId: 64 LOWERCASE hex chars (§3.1)

// A scratch dir with a couple of deterministic files for the behavioral cases.
let dir: string
let fileStoreUri: string
// A LARGE tree whose BLAKE3 walk takes measurable wall-clock — used by the
// genuine-concurrency event-loop test (§3) so the proof of off-thread execution
// does NOT depend on a resolution-ordering race (see that test for the rationale).
let bigDir: string
let bigWalkMs = 0

beforeAll(async () => {
  dir = mkdtempSync(join(tmpdir(), 'snapdir-node-spec-'))
  writeFileSync(join(dir, 'a.txt'), 'hello\n')
  mkdirSync(join(dir, 'sub'))
  writeFileSync(join(dir, 'sub', 'b.txt'), 'world\n')
  // A `file://` store URI to push into (a separate empty dir).
  const storeDir = mkdtempSync(join(tmpdir(), 'snapdir-node-store-'))
  fileStoreUri = pathToFileURL(storeDir).href // file:///… (StoreUri requires ://)

  // Build a tree big enough that the manifest (BLAKE3) walk takes tens of ms of
  // real wall-clock. 6000 × 8 KiB files across 60 dirs ⇒ a genuinely long walk
  // so we can observe whether the MAIN THREAD makes progress DURING it.
  bigDir = mkdtempSync(join(tmpdir(), 'snapdir-node-big-'))
  const NDIRS = 60
  for (let d = 0; d < NDIRS; d++) mkdirSync(join(bigDir, `d${d}`), { recursive: true })
  for (let i = 0; i < 6000; i++) {
    writeFileSync(join(bigDir, `d${i % NDIRS}`, `f${i}.bin`), Buffer.alloc(8192, i & 0xff))
  }
  // Warm + measure the bare walk duration once (so the concurrency test can
  // sanity-check that the walk really is long enough to observe progress).
  const t0 = performance.now()
  await manifest(bigDir)
  bigWalkMs = performance.now() - t0
})

afterAll(() => {
  if (dir) rmSync(dir, { recursive: true, force: true })
  if (bigDir) rmSync(bigDir, { recursive: true, force: true })
})

// ============================================================================
// 0. version() — the ONE export that exists today (sanity anchor).
// ============================================================================
describe('version()', () => {
  // Spec: PUBLIC_API.md §6 `version() -> &'static str` (SYNC, infallible).
  it('is a sync non-empty string', () => {
    const v = version()
    expectTypeOf(version).returns.toEqualTypeOf<string>()
    expect(typeof v).toBe('string')
    expect(v.length).toBeGreaterThan(0)
    // Tracks the CLI version (1.10.0 lineage) — pin shape, not the exact value.
    expect(v).toMatch(/^\d+\.\d+\.\d+/)
  })
})

// ============================================================================
// 1. ManifestEntry.size is `bigint` — THE headline Node-idiom requirement.
//    Plus the rest of the typed Manifest/ManifestEntry shape (§3.2).
// ============================================================================
describe('ManifestEntry typed shape (size: bigint headline)', () => {
  // Spec: PUBLIC_API.md §3.2 — size:u64 → MUST be `bigint` in Node (a JS
  // `number` cannot hold u64 sizes). node.md: "size is bigint, always".
  it('pins ManifestEntry field types — size is bigint, NOT number', () => {
    expectTypeOf<ManifestEntry['size']>().toEqualTypeOf<bigint>()
    // size must NOT be a plain JS number (would silently truncate u64).
    expectTypeOf<ManifestEntry['size']>().not.toEqualTypeOf<number>()

    // path: PathBuf → string in JS.
    expectTypeOf<ManifestEntry['path']>().toEqualTypeOf<string>()

    // permissions: u32 octal bits → a JS number is safe (fits in 53-bit).
    expectTypeOf<ManifestEntry['permissions']>().toEqualTypeOf<number>()

    // checksum: [u8;32] BLAKE3 bytes. Exposed to JS as a 64-hex string OR a
    // Buffer/Uint8Array. The contract: it is a fixed-width content checksum;
    // pin that it is present and string-or-bytes (the impl picks one — most
    // Node-idiomatic is the lowercase-hex string).
    expectTypeOf<ManifestEntry['checksum']>().not.toBeUndefined()

    // path_type / pathType → the PathType enum ('File' | 'Directory' shape).
    expectTypeOf<ManifestEntry['pathType']>().toEqualTypeOf<PathType>()
  })

  it('PathType is the File|Directory union (§3.3)', () => {
    // napi enums surface as string-literal unions; pin the two members.
    expectTypeOf<PathType>().toEqualTypeOf<'File' | 'Directory'>()
  })

  it('Manifest carries entries[] + the raw manifest text (§3.2)', () => {
    expectTypeOf<Manifest['entries']>().toEqualTypeOf<ManifestEntry[]>()
    expectTypeOf<Manifest['raw']>().toEqualTypeOf<string>()
  })

  it('a real manifest() entry has a bigint size at RUNTIME', async () => {
    // Behavioral: not just the type — the actual runtime value is a bigint.
    const m = await manifest(dir)
    expect(Array.isArray(m.entries)).toBe(true)
    expect(typeof m.raw).toBe('string')
    const fileEntry = m.entries.find((e) => e.path.endsWith('a.txt'))
    expect(fileEntry).toBeDefined()
    // "hello\n" = 6 bytes; the size MUST be a bigint, not a number.
    expect(typeof fileEntry!.size).toBe('bigint')
    expect(fileEntry!.size).toBe(6n)
    // Defensive: it is a TRUE bigint primitive, not a Number that merely prints
    // like one — `6 === 6n` is false in JS, so a Number coercion would fail this.
    expect((fileEntry!.size as unknown) === 6).toBe(false)

    // RUNTIME-vs-d.ts (Flag 2): the hand-patched d.ts types `permissions: number`,
    // `checksum: string`, `pathType: PathType`. Confirm the ACTUAL values match —
    // a d.ts that lies about these would be caught here.
    expect(typeof fileEntry!.permissions).toBe('number')
    expect(Number.isInteger(fileEntry!.permissions)).toBe(true)
    expect(typeof fileEntry!.checksum).toBe('string')
    expect(fileEntry!.checksum).toMatch(HEX64) // 64-lowercase-hex BLAKE3
    expect(['File', 'Directory']).toContain(fileEntry!.pathType)
    expect(fileEntry!.pathType).toBe('File')
    const dirEntry = m.entries.find((e) => e.pathType === 'Directory')
    expect(dirEntry).toBeDefined() // the snapshot root is a Directory entry
  })

  it('size is EXACT bigint for a multi-MB file and survives >2^53 arithmetic', async () => {
    // The WHOLE reason size is bigint (not number): u64 sizes exceed
    // Number.MAX_SAFE_INTEGER (2^53-1). We cannot cheaply make a >9 PB file, so we
    // pin the two properties that a >2^53 size depends on:
    //   (a) an EXACT large value is preserved with no Number() truncation, and
    //   (b) the value is a real bigint that stays exact past 2^53 under arithmetic.
    const big = mkdtempSync(join(tmpdir(), 'snapdir-node-bigfile-'))
    const SIZE = 3_000_017 // odd ⇒ no power-of-two rounding can hide a truncation
    writeFileSync(join(big, 'big.bin'), Buffer.alloc(SIZE, 7))
    const m = await manifest(big)
    const e = m.entries.find((x) => x.path.endsWith('big.bin'))!
    expect(typeof e.size).toBe('bigint')
    // Exact equality — a Number round-trip on an odd value would not be reproduced
    // here, but more importantly the type is bigint and the value is precise.
    expect(e.size).toBe(3000017n)

    // bigint arithmetic on the reported size stays exact (it is a real bigint,
    // not a Number that merely prints like one).
    const sq = e.size * e.size
    expect(typeof sq).toBe('bigint')
    expect(sq).toBe(9000102000289n) // 3000017^2, exact

    // Past 2^53 (Number.MAX_SAFE_INTEGER): add 2^53 to the real reported size and
    // prove the result is exact as a bigint but WOULD be truncated by Number().
    // This is the load-bearing property the whole "size is bigint" idiom exists
    // for — u64 sizes above 2^53 must round-trip without loss.
    const past53 = e.size + 2n ** 53n // 2^53 + 3000017 = 9007199257741009
    expect(past53).toBe(9007199257741009n)
    expect(past53).toBe(2n ** 53n + 3000017n) // exact, independent of the literal
    expect(past53 > BigInt(Number.MAX_SAFE_INTEGER)).toBe(true)
    // A JS Number cannot hold this exactly: the round-trip loses precision,
    // whereas the bigint did not. Pin both halves of that contrast.
    expect(BigInt(Number(past53)) === past53).toBe(false) // Number() WOULD truncate
    rmSync(big, { recursive: true, force: true })
  })
})

// ============================================================================
// 2. SnapdirError instanceof Error, non-empty .code ∈ the 8 frozen codes.
// ============================================================================
describe('SnapdirError (§4)', () => {
  // Spec: §4.1 — every binding maps .code() → its native error subtype; the 8
  // code strings are frozen. node.md: "SnapdirError extends Error with a
  // non-empty .code string".
  it('SnapdirError is a subclass of Error (type-level)', () => {
    expectTypeOf<SnapdirError>().toMatchTypeOf<Error>()
    expectTypeOf<SnapdirError['code']>().toEqualTypeOf<string>()
  })

  it('a failing call rejects with a SnapdirError: instanceof Error + non-empty .code', async () => {
    // id() on a non-existent path → an IO_ERROR-class failure.
    const missing = join(dir, 'does', 'not', 'exist')
    let caught: unknown
    try {
      await id(missing)
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SnapdirError)
    expect(caught).toBeInstanceOf(Error) // SnapdirError extends Error
    const err = caught as SnapdirError
    expect(typeof err.code).toBe('string')
    expect(err.code.length).toBeGreaterThan(0) // NON-EMPTY
    expect(STABLE_CODES).toContain(err.code as StableCode)
    // A missing path is an I/O failure.
    expect(err.code).toBe('IO_ERROR')
    // .message is a real, non-empty Error message (stable Display string).
    expect(typeof err.message).toBe('string')
    expect(err.message.length).toBeGreaterThan(0)
  })

  it('a malformed snapshot id rejects with INVALID_ID (§3.1 from_hex)', async () => {
    // idFromManifest is pure, but the id-parsing surface (fetch/checkout taking
    // an id string) must reject a non-hex/short id with INVALID_ID. Use the
    // most direct id-consuming path available: a verify against a bogus id.
    let caught: unknown
    try {
      await verify('not-a-valid-hex-id', fileStoreUri)
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SnapdirError)
    expect((caught as SnapdirError).code).toBe('INVALID_ID')
  })

  it('a malformed store URI rejects with INVALID_STORE (§3.4)', async () => {
    // StoreUri is stricter than RFC: `file:/missing-slashes` (no `://`) is
    // rejected with INVALID_STORE (§9.2).
    let caught: unknown
    try {
      await push(dir, 'file:/missing-slashes')
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SnapdirError)
    expect((caught as SnapdirError).code).toBe('INVALID_STORE')
  })

  it('each publicly-reachable error path pins its EXACT .code string (runtime)', async () => {
    // Strengthen: trigger several distinct stable codes and pin the exact string
    // each surfaces at RUNTIME (not just "some code"). This guards against the
    // wrapper collapsing every failure into one bucket.
    async function codeOf(fn: () => Promise<unknown>): Promise<string> {
      try {
        await fn()
      } catch (e) {
        expect(e).toBeInstanceOf(SnapdirError)
        expect(e).toBeInstanceOf(Error)
        const c = (e as SnapdirError).code
        expect(typeof c).toBe('string')
        expect(STABLE_CODES).toContain(c as StableCode)
        return c
      }
      throw new Error('expected the call to reject, but it resolved')
    }
    // IO_ERROR — id() of a missing path.
    expect(await codeOf(() => id(join(dir, 'no', 'such', 'path')))).toBe('IO_ERROR')
    // INVALID_ID — a non-hex id handed to an id-consuming fn.
    expect(await codeOf(() => verify('not-a-valid-hex-id', fileStoreUri))).toBe('INVALID_ID')
    // INVALID_ID — also via fetch (a too-short id).
    expect(await codeOf(() => snapdirFetch('zz', fileStoreUri))).toBe('INVALID_ID')
    // INVALID_STORE — a malformed store URI (no `://`).
    expect(await codeOf(() => push(dir, 'file:/missing-slashes'))).toBe('INVALID_STORE')
    // STORE_ERROR — checkout of a well-formed but absent snapshot id.
    expect(await codeOf(() => checkout('a'.repeat(64), join(tmpdir(), `co-${Date.now()}`)))).toBe(
      'STORE_ERROR'
    )
  })

  it('all 8 codes are representable on the type (frozen contract)', () => {
    // Pin that .code is assignable from each frozen code (the union is open —
    // .code is `string` — but every binding MUST be able to surface all 8).
    for (const c of STABLE_CODES) {
      expectTypeOf(c).toMatchTypeOf<SnapdirError['code']>()
    }
    expect(STABLE_CODES).toHaveLength(8)
  })
})

// ============================================================================
// 3. Async fns return Promises and never block the event loop.
// ============================================================================
describe('async surface returns Promises (§6 + node.md thread-pool)', () => {
  // Spec: PUBLIC_API.md §6 — push/fetch/pull/checkout/sync/diff/verify are
  // async; node.md projects manifest/id/stage as async too (CPU walk on a
  // thread pool). Each returns a Promise; awaiting never blocks libuv.

  it('manifest/id/stage are async (Promise-returning) — node.md', () => {
    expectTypeOf(manifest).returns.resolves.toEqualTypeOf<Manifest>()
    expectTypeOf(id).returns.resolves.toEqualTypeOf<SnapshotId>()
    expectTypeOf(stage).returns.resolves.toEqualTypeOf<SnapshotId>()
  })

  it('distribution fns are async (Promise-returning) — §6', () => {
    expectTypeOf(push).returns.resolves.toEqualTypeOf<SnapshotId>()
    expectTypeOf(snapdirFetch).returns.resolves.toEqualTypeOf<void>()
    expectTypeOf(pull).returns.resolves.toEqualTypeOf<void>()
    expectTypeOf(checkout).returns.resolves.toEqualTypeOf<void>()
    expectTypeOf(sync).returns.resolves.toEqualTypeOf<void>()
    expectTypeOf(diff).returns.resolves.toEqualTypeOf<DiffEntry[]>()
    // verify resolves to a result with at least an `ok: boolean` (§3.8).
    expectTypeOf(verify).returns.resolves.toHaveProperty('ok')
  })

  it('a returned value is a real thenable Promise at runtime', () => {
    const p = manifest(dir)
    expect(p).toBeInstanceOf(Promise)
    expect(typeof (p as Promise<unknown>).then).toBe('function')
    return p // settle it so no open handle leaks
  })

  // --------------------------------------------------------------------------
  // GENUINE CONCURRENCY (tests-review, opus — strengthened).
  //
  // The ORIGINAL spec test here set a 0ms timer, `await manifest(dir)` on a TINY
  // tree, then asserted the timer had fired. That is a RESOLUTION-ORDERING race:
  // it only proves "the 0ms timer beat manifest's resolution", which is trivially
  // satisfiable WITHOUT any concurrency by deferring manifest's resolution one
  // macrotask tick (a `setTimeout(resolve,0)` in the wrapper). It would PASS even
  // if the walk were fully BLOCKING — it masks the very property under test, and
  // the defer it rewards adds a macrotask of latency to every async call.
  //
  // These rewritten tests prove off-thread execution DIRECTLY: while a long walk
  // (the 6000-file `bigDir`) is in flight, the MAIN THREAD must keep making
  // progress. A blocking walk would freeze the main thread for the whole walk →
  // ZERO progress. A `setTimeout(resolve,0)` defer of a blocking walk does NOT
  // satisfy these — the main thread is still frozen during compute(). Only a
  // genuinely off-thread AsyncTask (libuv thread pool / spawn_blocking) passes.
  // --------------------------------------------------------------------------

  it('main-thread timers keep firing DURING a long manifest walk (off-thread)', async () => {
    // Sanity: the warm-up walk in beforeAll must have been long enough to observe
    // concurrency on. If it weren't, this test would be vacuous.
    expect(bigWalkMs).toBeGreaterThan(8) // tens of ms in practice; floor is generous

    let ticks = 0
    const iv = setInterval(() => {
      ticks++
    }, 1)
    const m = await manifest(bigDir)
    clearInterval(iv)

    // A BLOCKING walk would freeze the main loop → the 1ms interval could not
    // fire at all while awaiting. Off-thread execution lets the timers phase run.
    // Require several ticks (not just ≥1) so a single stray pre/post-await tick
    // can't satisfy it — these must have fired *during* the walk.
    expect(ticks).toBeGreaterThanOrEqual(3)
    expect(m.entries.length).toBeGreaterThan(0)
  })

  it('a CPU-light main-thread loop makes progress DURING a long manifest walk', async () => {
    // A self-rescheduling setImmediate loop is the most sensitive probe: it runs
    // once per event-loop iteration. If manifest blocked the main thread, the loop
    // body would not execute again until the walk finished → progress stays at its
    // pre-await value. Off-thread execution lets thousands of iterations run.
    let progress = 0
    let running = true
    const loop = () => {
      if (running) {
        progress++
        setImmediate(loop)
      }
    }
    setImmediate(loop)

    const m = await manifest(bigDir)
    running = false

    // Off-thread ⇒ the main loop spun many times while the walk ran. This is NOT
    // satisfiable by a `setTimeout(resolve,0)` defer of a blocking walk (the defer
    // only moves the *final* resolution one tick; it cannot un-block compute()).
    expect(progress).toBeGreaterThanOrEqual(50)
    expect(m.entries.length).toBeGreaterThan(0)
  })

  it('id() and stage() are also genuinely off-thread (not just manifest)', async () => {
    // Pin the property for the OTHER two AsyncTask-backed fns too, so the impl
    // can't make only `manifest` concurrent.
    for (const fn of [id, stage] as const) {
      let progress = 0
      let running = true
      const loop = () => {
        if (running) {
          progress++
          setImmediate(loop)
        }
      }
      setImmediate(loop)
      await fn(bigDir)
      running = false
      expect(progress).toBeGreaterThanOrEqual(50)
    }
  })

  it('no async/native handle leaks after the async surface is exercised', async () => {
    // The spec NOTE asks for a `--detect-open-handles`-clean run (no leaked tokio
    // runtime / napi worker). vitest's flag is process-global, so we assert the
    // invariant directly: after awaiting the async fns and letting microtasks +
    // one macrotask drain, no extra napi/tokio handle should remain registered.
    const before = process.getActiveResourcesInfo().length
    await manifest(dir)
    await id(dir)
    await stage(dir)
    // Let any deferred resolution macrotask + the next timers phase drain.
    await new Promise<void>((r) => setTimeout(r, 5))
    const after = process.getActiveResourcesInfo().length
    // A leaked per-call handle would make `after` grow past `before`.
    expect(after).toBeLessThanOrEqual(before)
  })
})

// ============================================================================
// 4. idFromManifest is SYNC + self-consistent with id().
// ============================================================================
describe('idFromManifest is SYNC (§6 id_from_manifest, pure/infallible)', () => {
  // Spec: §6 `id_from_manifest(m) -> SnapshotId` — pure, no I/O, infallible.
  it('returns a SnapshotId synchronously (NOT a Promise)', () => {
    expectTypeOf(idFromManifest).returns.toEqualTypeOf<SnapshotId>()
    // SnapshotId is a hex string at the value level.
    expectTypeOf<SnapshotId>().toEqualTypeOf<string>()
    expectTypeOf(idFromManifest).parameter(0).toEqualTypeOf<Manifest>()
  })

  it('idFromManifest(manifest(dir)) === id(dir) (self-consistency, §9.1)', async () => {
    const m = await manifest(dir)
    const derived = idFromManifest(m) // SYNC — no await
    expect(typeof derived).toBe('string')
    expect(derived).toMatch(HEX64) // 64 lowercase hex
    const direct = await id(dir)
    // The id is BLAKE3-of-manifest-text, so the two MUST agree byte-for-byte
    // (id ignores checksum_bin; §9.4). This is the cross-binding parity anchor.
    expect(derived).toBe(direct)
  })
})

// ============================================================================
// 5. ESM + CJS dual: both entrypoints expose the same surface.
// ============================================================================
describe('ESM + CJS dual output (node.md: dual exports)', () => {
  // Spec: package.json `exports` maps `import`→ESM and `require`→CJS; node.md:
  // "ESM + CJS dual output". Both must expose the identical surface.
  it('the ESM import exposes the full surface', () => {
    for (const fn of [
      version,
      manifest,
      id,
      idFromManifest,
      stage,
      push,
      snapdirFetch,
      pull,
      checkout,
      sync,
      diff,
      verify,
    ]) {
      expect(typeof fn).toBe('function')
    }
    expect(typeof SnapdirError).toBe('function') // a class
  })

  it("require('@snapdir/snapdir') (CJS) exposes the same exports as ESM", () => {
    const require = createRequire(import.meta.url)
    const cjs = require('@snapdir/snapdir')
    // Same key surface as the ESM import.
    for (const name of [
      'version',
      'manifest',
      'id',
      'idFromManifest',
      'stage',
      'push',
      'fetch',
      'pull',
      'checkout',
      'sync',
      'diff',
      'verify',
      'SnapdirError',
    ]) {
      expect(cjs).toHaveProperty(name)
      expect(typeof cjs[name]).toBe('function')
    }
    // CJS and ESM agree on version().
    expect(cjs.version()).toBe(version())
    // CJS SnapdirError is the same Error-subclass contract.
    expect(cjs.SnapdirError.prototype).toBeInstanceOf(Error)
  })
})

// ============================================================================
// 6. SnapshotId — a 64-lowercase-hex string; id()/idFromManifest() return it.
// ============================================================================
describe('SnapshotId is 64 lowercase hex (§3.1)', () => {
  it('id() returns a 64-hex SnapshotId string', async () => {
    expectTypeOf(id).returns.resolves.toEqualTypeOf<SnapshotId>()
    const sid = await id(dir)
    expect(typeof sid).toBe('string')
    expect(sid).toMatch(HEX64)
    // lowercase only (Display always emits lowercase, §3.1).
    expect(sid).toBe(sid.toLowerCase())
  })

  it('is deterministic for the same tree (idempotent re-walk)', async () => {
    const a = await id(dir)
    const b = await id(dir)
    expect(a).toBe(b)
  })
})

// ============================================================================
// 7. Behavioral round-trip: manifest → id → push (basic shape).
// ============================================================================
describe('round-trip behavioral shape (manifest → id → push)', () => {
  // Spec: §6 manifest/id/push. These will FAIL until the impl exists — correct.
  it('manifest(dir) → text; id(dir) → hex; push(dir, file://) → id', async () => {
    const m = await manifest(dir)
    expect(m.raw.length).toBeGreaterThan(0)
    expect(m.entries.length).toBeGreaterThan(0)

    const sid = await id(dir)
    expect(sid).toMatch(HEX64)

    // push returns the snapshot id of what it pushed; it must equal id(dir)
    // (push of a path walks the same tree → same id, §9.1 parity).
    const pushedId = await push(dir, fileStoreUri)
    // RUNTIME-vs-d.ts (Flag 2): the hand-patched index.d.ts types push() as
    // Promise<SnapshotId> and SnapshotId = string. Confirm the ACTUAL resolved
    // value is a real 64-lowercase-hex string — a lying d.ts is caught here.
    expect(typeof pushedId).toBe('string')
    expect(pushedId).toMatch(HEX64)
    expect(pushedId).toBe(pushedId.toLowerCase())
    expect(pushedId).toBe(sid)
  })

  it('diff() resolves to a DiffEntry[] whose status is EXACTLY a frozen glyph at RUNTIME (§3.6)', async () => {
    // DiffStatus glyphs are frozen single chars; pin the union shape at type level…
    expectTypeOf<DiffStatus>().toEqualTypeOf<'A' | 'D' | 'M' | '='>()
    expectTypeOf<DiffEntry['status']>().toEqualTypeOf<DiffStatus>()

    // …and confirm the RUNTIME value matches the hand-patched d.ts (Flag 2). The
    // gen-wrapper.cjs hand-edits DiffEntry.status from `string` to `DiffStatus`;
    // a real diff that EXERCISES the glyphs must only ever produce that frozen set
    // (no stray status like 'Added' / '~' leaking through the Rust→string map).
    const FROZEN = new Set(['A', 'D', 'M', '='])

    // Self-vs-self diff: must be empty or all-'=' — never a foreign glyph.
    const same = await diff({ from: [fileStoreUri], to: [fileStoreUri] })
    expect(Array.isArray(same)).toBe(true)
    for (const e of same) {
      expect(typeof e.path).toBe('string')
      expect(FROZEN.has(e.status)).toBe(true)
      expect(e.status.length).toBe(1) // single-char glyph, not a word
    }

    // Now produce a REAL 'A': push a base, add a file, push to a 2nd store, diff.
    const storeDirB = mkdtempSync(join(tmpdir(), 'snapdir-node-storeB-'))
    const storeUriB = pathToFileURL(storeDirB).href
    const diffDir = mkdtempSync(join(tmpdir(), 'snapdir-node-diff-'))
    writeFileSync(join(diffDir, 'keep.txt'), 'keep\n')
    await push(diffDir, fileStoreUri) // already pushed dir above; re-push base set
    const baseStore = mkdtempSync(join(tmpdir(), 'snapdir-node-base-'))
    const baseUri = pathToFileURL(baseStore).href
    await push(diffDir, baseUri)
    writeFileSync(join(diffDir, 'added.txt'), 'new\n') // an ADDED file
    await push(diffDir, storeUriB)

    const changed = await diff({ from: [baseUri], to: [storeUriB] })
    expect(Array.isArray(changed)).toBe(true)
    for (const e of changed) {
      expect(typeof e.path).toBe('string')
      expect(FROZEN.has(e.status)).toBe(true)
      expect(e.status.length).toBe(1)
    }
    // The added.txt must surface as an 'A' (added on the `to` side).
    const added = changed.find((e) => e.path.endsWith('added.txt'))
    expect(added).toBeDefined()
    expect(added!.status).toBe('A')

    rmSync(diffDir, { recursive: true, force: true })
  })
})
