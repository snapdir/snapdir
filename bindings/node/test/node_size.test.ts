/**
 * node_size.ts — BLACK-BOX spec for the `snapdir size` surface of the
 * @snapdir/snapdir Node (napi-rs) binding (Phase 46, gate
 * `node-size-spec-tests`, adversary/opus).
 *
 * ============================================================================
 * WHAT THIS PINS
 * ----------------------------------------------------------------------------
 * The `snapdir size` contract every Node USER sees, authored from the SPEC +
 * the binding's public surface (bindings/node/index.d.ts) ONLY. The three new
 * surface members this gate pins — none of which exist yet (that is the
 * intended no-impl / expected-fail state) — are:
 *
 *   • `SizeStats { logicalBytes: bigint; physicalBytes: bigint;
 *                  files: bigint; objects: bigint }`
 *       Every field is a BigInt. napi maps the Rust `u64` totals → JS BigInt
 *       because u64 exceeds Number.MAX_SAFE_INTEGER, exactly like the existing
 *       `ManifestEntry.size: bigint`. A napi `u32 → number` regression on
 *       `files`/`objects` MUST be caught here.
 *
 *   • `manifestSize(m: Manifest): SizeStats` — SYNC (returns a value directly,
 *       NOT a Promise). Pure over the already-parsed manifest; no I/O.
 *
 *   • `size(storeUri: string, id?: string): Promise<SizeStats>` — ASYNC.
 *       `id` omitted/undefined ⇒ the WHOLE store (all snapshots, deduped);
 *       a hex id ⇒ just that snapshot. Rejects with `SnapdirError` (`.code`)
 *       on a bad store.
 *
 * ----------------------------------------------------------------------------
 * SEMANTICS (objects are stored UNCOMPRESSED — the acceptance bar depends on it)
 * ----------------------------------------------------------------------------
 *   logicalBytes  = Σ size over ALL File entries      (duplicates counted)
 *   physicalBytes = Σ size over UNIQUE checksums      (dedup by checksum)
 *   files         = count of File entries
 *   objects       = count of distinct checksums
 *
 * Because objects are stored UNCOMPRESSED, `physicalBytes` MUST equal the
 * on-disk byte total of `<storeDir>/.objects/`. Case 4 walks that dir with
 * `node:fs` (readdirSync recursive + statSync sizes) and asserts equality —
 * the load-bearing acceptance test.
 *
 * ----------------------------------------------------------------------------
 * DETERMINISTIC FIXTURE (parity with the cpp / go / java / Rust suites)
 * ----------------------------------------------------------------------------
 *   seed/a/x.txt   = "hello world\n"     (12 bytes)
 *   seed/b/dup.txt = "hello world\n"     (12 bytes — DUPLICATE content)
 *   seed/a/y.txt   = "unique data here\n"(17 bytes)
 * ⇒ files=3n, objects=2n, logicalBytes=41n (12+12+17), physicalBytes=29n (12+17)
 *
 * ----------------------------------------------------------------------------
 * STORE-ERROR CONTRACT (SOURCE-CONFIRMED — do NOT weaken to absent==empty)
 * ----------------------------------------------------------------------------
 *   (a) MISSING-root file:// store (dir does NOT exist)  → REJECTS,
 *         SnapdirError.code === 'STORE_ERROR'  (missing ≠ empty)
 *   (b) EXISTING store dir with NO manifests (empty)     → RESOLVES to all-0n
 *   (c) malformed / unknown-scheme URI ('bogus://nope')  → REJECTS,
 *         SnapdirError.code === 'INVALID_STORE' (or at least a non-empty code)
 *
 * ----------------------------------------------------------------------------
 * EXPECTED-FAIL RATIONALE (this IS the no-impl state — correct & intended)
 * ----------------------------------------------------------------------------
 * `size`, `manifestSize`, and the `SizeStats` type do NOT exist in the current
 * index.d.ts. So `tsc --strict` errors on their imports and vitest cannot
 * resolve them at runtime. That failure is the no-impl signal. This file is
 * well-formed TS that WOULD typecheck + pass once the impl lands the surface;
 * the `-impl` gate `git mv`s it into bindings/node/test/ and makes it green.
 *
 * ----------------------------------------------------------------------------
 * BLACK-BOX ATTESTATION
 * ----------------------------------------------------------------------------
 * Authored from the SPEC + bindings/node/index.d.ts + the existing
 * bindings/node/test/node_api.test.ts conventions ONLY. I did NOT read
 * crates/snapdir-ffi/src/ or crates/snapdir-api/src/ or any Rust internals.
 * ============================================================================
 */

import { describe, it, expect, expectTypeOf, beforeAll, afterAll } from 'vitest'
import { mkdtempSync, writeFileSync, mkdirSync, rmSync, readdirSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL, fileURLToPath } from 'node:url'

// The binding under test (ESM import). `manifestSize`, `size`, and the
// `SizeStats` type do NOT exist until the impl gate — every one of those
// imports is the intended no-impl tsc error.
import {
  manifest,
  push,
  SnapdirError,
  // NEW surface this gate pins (does not exist yet):
  manifestSize,
  size,
  type Manifest,
  type SizeStats,
} from '@snapdir/snapdir'

// The 8 FROZEN cross-language error codes (PUBLIC_API.md §4.1). A SnapdirError
// .code MUST be exactly one of these strings.
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

// The deterministic figure the whole suite pivots on (BigInt everywhere).
const EXPECTED: SizeStats = {
  logicalBytes: 41n, // 12 + 12 + 17
  physicalBytes: 29n, // 12 + 17 (dup collapsed)
  files: 3n,
  objects: 2n,
}

// --- fixture state -----------------------------------------------------------
let seedDir: string // the deterministic a/x, b/dup, a/y tree
let storeDir: string // file:// store we push the seed into
let storeUri: string // pathToFileURL(storeDir).href
let pushedId: string // snapshot id returned by push(seedDir, storeUri)

let emptyStoreDir: string // an EXISTING store dir, nothing pushed (case b)
let emptyStoreUri: string
let missingStoreUri: string // a file:// URL whose dir does NOT exist (case a)

// Recursively sum the byte size of every regular file under `<storeDir>/.objects/`.
// This is the on-disk PHYSICAL total the acceptance test (case 4) compares
// `size().physicalBytes` against. Objects are stored UNCOMPRESSED, so the walk
// of the raw object files must equal Σ(unique-checksum sizes) exactly.
function onDiskObjectsBytes(storePath: string): bigint {
  const objectsRoot = join(storePath, '.objects')
  let total = 0n
  // readdirSync(..., { recursive: true }) yields every nested entry; statSync
  // isFile() filters out the intermediate fan-out directories.
  for (const rel of readdirSync(objectsRoot, { recursive: true }) as string[]) {
    const full = join(objectsRoot, rel)
    const st = statSync(full)
    if (st.isFile()) total += BigInt(st.size)
  }
  return total
}

beforeAll(async () => {
  // Deterministic fixture — byte-for-byte parity with cpp/go/java/Rust suites.
  seedDir = mkdtempSync(join(tmpdir(), 'snapdir-node-size-seed-'))
  mkdirSync(join(seedDir, 'a'), { recursive: true })
  mkdirSync(join(seedDir, 'b'), { recursive: true })
  writeFileSync(join(seedDir, 'a', 'x.txt'), 'hello world\n') // 12
  writeFileSync(join(seedDir, 'b', 'dup.txt'), 'hello world\n') // 12 (DUP content)
  writeFileSync(join(seedDir, 'a', 'y.txt'), 'unique data here\n') // 17

  // Push the seed into a fresh file:// store; keep the raw path AND the URI.
  storeDir = mkdtempSync(join(tmpdir(), 'snapdir-node-size-store-'))
  storeUri = pathToFileURL(storeDir).href
  pushedId = await push(seedDir, storeUri)

  // Case (b): an EXISTING store dir with NOTHING pushed → must resolve to 0n.
  emptyStoreDir = mkdtempSync(join(tmpdir(), 'snapdir-node-size-empty-'))
  emptyStoreUri = pathToFileURL(emptyStoreDir).href

  // Case (a): a file:// URL whose directory does NOT exist. Build a URL from a
  // never-created path so the root is genuinely absent (not merely empty).
  const missingPath = join(tmpdir(), `snapdir-node-size-missing-${Date.now()}-${process.pid}`)
  missingStoreUri = pathToFileURL(missingPath).href
})

afterAll(() => {
  for (const d of [seedDir, storeDir, emptyStoreDir]) {
    if (d) rmSync(d, { recursive: true, force: true })
  }
})

// ============================================================================
// 1. manifestSize(manifest(seed)) — the deterministic figure + BigInt type-pin.
// ============================================================================
describe('manifestSize() — SYNC over a parsed Manifest', () => {
  it('deep-equals {41n,29n,3n,2n} and every field is a BigInt', async () => {
    // clause: SizeStats is SYNC (no Promise) and pure over the manifest.
    const m = await manifest(seedDir)
    const result = manifestSize(m) // SYNC — no await

    // clause: deep-equals the deterministic fixture figure, exactly.
    expect(result).toEqual(EXPECTED)

    // clause: type-pin — each field is `bigint` on the SizeStats type.
    expectTypeOf<SizeStats['logicalBytes']>().toEqualTypeOf<bigint>()
    expectTypeOf<SizeStats['physicalBytes']>().toEqualTypeOf<bigint>()
    expectTypeOf<SizeStats['files']>().toEqualTypeOf<bigint>()
    expectTypeOf<SizeStats['objects']>().toEqualTypeOf<bigint>()
    // clause: none of the totals is a plain JS `number` (would truncate u64).
    expectTypeOf<SizeStats['files']>().not.toEqualTypeOf<number>()

    // clause: manifestSize is SYNC — its return type is SizeStats, not a Promise.
    expectTypeOf(manifestSize).returns.toEqualTypeOf<SizeStats>()
    expectTypeOf(manifestSize).parameter(0).toEqualTypeOf<Manifest>()

    // clause: RUNTIME — every value is a real bigint primitive, not a Number.
    expect(typeof result.logicalBytes).toBe('bigint')
    expect(typeof result.physicalBytes).toBe('bigint')
    expect(typeof result.files).toBe('bigint')
    expect(typeof result.objects).toBe('bigint')
    // clause: a Number that merely prints like 41 would fail 41 === 41n.
    expect((result.logicalBytes as unknown) === 41).toBe(false)
  })
})

// ============================================================================
// 2. size(storeUri, id) — a specific snapshot equals the manifestSize figure.
// ============================================================================
describe('size(storeUri, id) — a single snapshot', () => {
  it('deep-equals the manifestSize figure for the pushed snapshot', async () => {
    // clause: size is ASYNC — returns a Promise<SizeStats>.
    expectTypeOf(size).returns.resolves.toEqualTypeOf<SizeStats>()
    const result = await size(storeUri, pushedId)
    // clause: an id-scoped size == the pure manifestSize over the same tree.
    expect(result).toEqual(EXPECTED)
  })
})

// ============================================================================
// 3. size(storeUri) — no id ⇒ WHOLE store (here 1 snapshot) == the single figure.
// ============================================================================
describe('size(storeUri) — whole store (id omitted)', () => {
  it('deep-equals the single figure; physicalBytes is 29n absolutely', async () => {
    // clause: omitting `id` sizes the whole (deduped) store; with one snapshot
    // pushed that equals the single-snapshot figure.
    const whole = await size(storeUri)
    expect(whole).toEqual(EXPECTED)
    // clause: physicalBytes is the deduped on-object total — exactly 29n.
    expect(whole.physicalBytes).toBe(29n)
  })
})

// ============================================================================
// 4. ACCEPTANCE — physicalBytes === on-disk `.objects/` byte total === 29n.
// ============================================================================
describe('ACCEPTANCE: physicalBytes == on-disk .objects/ total', () => {
  it('size().physicalBytes equals the walked .objects/ byte sum (== 29n)', async () => {
    // clause: objects are stored UNCOMPRESSED, so the raw on-disk object bytes
    // must equal the reported physicalBytes. Recover the store path from the
    // file:// URI (fileURLToPath) and walk <storeDir>/.objects/.
    const storePath = fileURLToPath(storeUri)
    const onDisk = onDiskObjectsBytes(storePath)

    const reported = (await size(storeUri)).physicalBytes
    // clause: reported physicalBytes === the on-disk sum, BigInt-exact.
    expect(reported).toBe(onDisk)
    // clause: and both are the deterministic 29n (12 + 17, dup collapsed).
    expect(onDisk).toBe(29n)
    expect(reported).toBe(29n)
  })
})

// ============================================================================
// 5. Strict invariants: logicalBytes > physicalBytes AND objects < files.
// ============================================================================
describe('strict dedup invariants (BigInt compare)', () => {
  it('logicalBytes > physicalBytes and objects < files', async () => {
    const result = await size(storeUri)
    // clause: a duplicate exists ⇒ logical (dups counted) > physical (deduped).
    expect(result.logicalBytes > result.physicalBytes).toBe(true)
    // clause: a duplicate exists ⇒ fewer distinct objects than File entries.
    expect(result.objects < result.files).toBe(true)
  })
})

// ============================================================================
// 6. STORE-ERROR contract — (a) missing-root, (b) empty, (c) malformed.
// ============================================================================
describe('store-error contract (a/b/c)', () => {
  it('(a) MISSING-root file:// store REJECTS with STORE_ERROR (not empty)', async () => {
    // clause: a missing store root does NOT masquerade as an empty store — it
    // rejects with a SnapdirError whose .code is exactly STORE_ERROR.
    let caught: unknown
    try {
      await size(missingStoreUri)
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SnapdirError)
    expect(caught).toBeInstanceOf(Error) // SnapdirError extends Error
    expect((caught as SnapdirError).code).toBe('STORE_ERROR')
    // Belt-and-suspenders on the rejection itself.
    await expect(size(missingStoreUri)).rejects.toBeInstanceOf(SnapdirError)
  })

  it('(b) EXISTING empty store RESOLVES to all-0n (no reject)', async () => {
    // clause: an existing store dir with no manifests is genuinely empty ⇒
    // resolves to all-zero SizeStats, never rejects.
    const zero = await size(emptyStoreUri)
    expect(zero).toEqual({
      logicalBytes: 0n,
      physicalBytes: 0n,
      files: 0n,
      objects: 0n,
    })
    // clause: the zeros are BigInt, not number.
    expect(typeof zero.files).toBe('bigint')
    expect(typeof zero.physicalBytes).toBe('bigint')
  })

  it('(c) malformed / unknown-scheme URI REJECTS with a stable code', async () => {
    // clause: a bogus scheme is not a valid StoreUri ⇒ rejects with a
    // SnapdirError carrying a non-empty stable code (INVALID_STORE expected).
    let caught: unknown
    try {
      await size('bogus://nope')
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SnapdirError)
    const code = (caught as SnapdirError).code
    expect(typeof code).toBe('string')
    expect(code.length).toBeGreaterThan(0) // NON-EMPTY stable code
    expect(STABLE_CODES).toContain(code as StableCode)
    expect(code).toBe('INVALID_STORE')
  })
})

// ============================================================================
// 7. BigInt-not-Number regression guard on size()'s runtime fields.
// ============================================================================
describe('napi u64 → BigInt (NOT u32 → number) regression guard', () => {
  it('size().files is a bigint at RUNTIME', async () => {
    // clause: a napi `u32 → number` mapping on the `files` counter would make
    // this a `number`; it MUST be a BigInt (all four totals are u64/BigInt).
    const result = await size(storeUri)
    expect(typeof result.files).toBe('bigint')
    expect(typeof result.objects).toBe('bigint')
    expect(typeof result.logicalBytes).toBe('bigint')
    expect(typeof result.physicalBytes).toBe('bigint')
    // clause: value + type both — files is exactly 3n as a bigint.
    expect(result.files).toBe(3n)
    expect((result.files as unknown) === 3).toBe(false)
  })
})

// ============================================================================
// 8. Degenerate: manifestSize of a dirs-only Manifest ⇒ all-0n.
// ============================================================================
describe('degenerate: dirs-only manifest', () => {
  it('manifestSize of a tree with NO File entries is all-0n', async () => {
    // clause: a manifest with only Directory entries has no File bytes and no
    // checksummed objects ⇒ every total is 0n.
    const dirsOnly = mkdtempSync(join(tmpdir(), 'snapdir-node-size-dirs-'))
    mkdirSync(join(dirsOnly, 'empty-a'), { recursive: true })
    mkdirSync(join(dirsOnly, 'empty-b', 'nested'), { recursive: true })
    const m = await manifest(dirsOnly)
    const result = manifestSize(m)
    expect(result).toEqual({
      logicalBytes: 0n,
      physicalBytes: 0n,
      files: 0n,
      objects: 0n,
    })
    rmSync(dirsOnly, { recursive: true, force: true })
  })
})

// ============================================================================
// 9. HARDENING (adversary review add) — cross-snapshot whole-store dedup.
//    Two independent snapshots share the 12-byte 'hello world\n' object; the
//    whole-store size MUST dedup it (physicalBytes/objects STRICTLY less than
//    the per-snapshot sums) while logicalBytes/files sum EXACTLY. All reference
//    figures are COMPUTED from the local manifests — no hardcoded byte counts.
// ============================================================================
describe('HARDENING: cross-snapshot whole-store dedup', () => {
  it('cross-snapshot dedup: whole-store dedups a shared object across snapshots', async () => {
    // seedA is the existing deterministic fixture (contains a 'hello world\n' object).
    const seedA = seedDir
    // seedB: a FRESH tree — one file that byte-for-byte SHARES seedA's 12-byte
    // 'hello world\n' object, plus one file with brand-new content.
    const seedB = mkdtempSync(join(tmpdir(), 'snapdir-node-size-seedB-'))
    writeFileSync(join(seedB, 'shared.txt'), 'hello world\n') // shares seedA's 12-byte object
    writeFileSync(join(seedB, 'fresh.txt'), 'brand new content!\n') // distinct object

    // Push BOTH into the ONE shared store (push(path, storeUri) — real signature).
    const idA = await push(seedA, storeUri)
    const idB = await push(seedB, storeUri)

    // clause: distinct trees ⇒ distinct snapshot ids.
    expect(idA).not.toBe(idB)

    // COMPUTED reference figures from the local manifests (no hardcoded bytes).
    const a = manifestSize(await manifest(seedA))
    const b = manifestSize(await manifest(seedB))

    // clause: each id-scoped size deep-equals its own COMPUTED manifestSize figure.
    expect(await size(storeUri, idA)).toEqual(a)
    expect(await size(storeUri, idB)).toEqual(b)

    const whole = await size(storeUri)

    // clause: logical totals + file counts SUM exactly (BigInt add, dups counted).
    expect(whole.files).toBe(a.files + b.files)
    expect(whole.logicalBytes).toBe(a.logicalBytes + b.logicalBytes)

    // clause: STRICT dedup — the shared object is stored ONCE, so the whole-store
    // physical bytes and object count are STRICTLY LESS than the per-snapshot sums.
    expect(whole.physicalBytes < a.physicalBytes + b.physicalBytes).toBe(true)
    expect(whole.objects < a.objects + b.objects).toBe(true)

    // clause: whole-store physicalBytes still equals the raw on-disk .objects/ sum.
    const storePath = fileURLToPath(storeUri)
    expect(whole.physicalBytes).toBe(onDiskObjectsBytes(storePath))

    // clause: re-run determinism — sizing the same store twice is byte-identical.
    expect(await size(storeUri)).toEqual(whole)

    // clause: BigInt-idiom pin (u64 → BigInt, guards a Number regression) — the
    // single-snapshot logical total is the value 41 yet the field itself is a
    // real bigint (a Number 41 would satisfy the value check but fail the type).
    expect(typeof a.logicalBytes).toBe('bigint')
    expect(Number(a.logicalBytes)).toBe(41)
    expectTypeOf(a.logicalBytes + 1n).toEqualTypeOf<bigint>()
    expect((a.logicalBytes as unknown) === 41).toBe(false)

    rmSync(seedB, { recursive: true, force: true })
  })
})
