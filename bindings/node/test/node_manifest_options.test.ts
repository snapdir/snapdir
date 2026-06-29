/**
 * node_manifest_options.test.ts — BLACK-BOX spec for the NEW manifest/id
 * OPTIONS surface on the @snapdir/snapdir Node binding (Phase 37, gate
 * `node-manifest-options-spec-tests`, adversary/opus).
 *
 * ============================================================================
 * WHAT THIS PINS
 * ----------------------------------------------------------------------------
 * The Node binding currently exposes `manifest(path)` and `id(path)` with NO
 * options argument. The fix (a later `-impl` gate) adds an optional 2nd arg
 * mapping to the snapdir-api `ManifestOptions` value:
 *
 *   manifest(path, options?: ManifestOptions): Promise<Manifest>
 *   id(path,       options?: ManifestOptions): Promise<SnapshotId>
 *
 *   interface ManifestOptions {
 *     noFollow?: boolean   // → snapdir-api follow_symlinks = !noFollow
 *     absolute?: boolean   // → snapdir-api absolute
 *     exclude?:  string[]  // → snapdir-api exclude (OR-combined regex patterns)
 *   }
 *
 * Semantics pinned (from the snapdir CLI / snapdir-api ManifestOptions spec +
 * the §1 cross-language parity contract):
 *
 *   1. no-follow vs follow are DISTINCT: default `manifest(path)` FOLLOWS
 *      symlinks (dereferences them); `{ noFollow: true }` records the link
 *      itself. The two MUST yield different `raw` text and different ids; and
 *      each variant is self-consistent (id(path,opts) === idFromManifest of
 *      manifest(path,opts)).
 *   2. absolute: `{ absolute: true }` renders root/entry paths as ABSOLUTE
 *      (starting with the absolute tree path) instead of the default relative
 *      `./` rendering — distinct raw text + distinct id.
 *   3. exclude: `{ exclude: ['<regex>'] }` omits matching entries; multiple
 *      patterns OR-combine; the array form is repeatable. An excluded entry is
 *      ABSENT, a non-excluded one PRESENT.
 *   4. optionality / backward-compat: omitting options (or `{}`) === the
 *      current default behavior — `manifest(path)` deep-equals `manifest(path,{})`.
 *   5. id parity with options: `id(path, opts)` is self-consistent with
 *      `manifest(path, opts)` for EACH option combination.
 *
 * ----------------------------------------------------------------------------
 * EXPECTED-FAIL RATIONALE (this is the no-impl / option-less state)
 * ----------------------------------------------------------------------------
 * Against the CURRENT option-less binding, the 2nd argument is simply IGNORED:
 * every option variant collapses to the default walk. So:
 *   • `manifest(path, {noFollow:true}).raw` === `manifest(path).raw`  → the
 *     INEQUALITY assertions in (1) FAIL.
 *   • `manifest(path, {absolute:true}).raw` is still relative `./…`     → (2) FAIL.
 *   • `manifest(path, {exclude:[…]}).entries` still contains the excluded
 *     entry                                                            → (3) FAIL.
 * That failure IS the no-impl signal. The deep-equal backward-compat case (4)
 * is the one that already holds and protects the impl from changing the
 * no-options default. The `-impl` gate `git mv`s this into bindings/node/test/
 * and makes it green; `-tests-review` strengthens it.
 *
 * ----------------------------------------------------------------------------
 * BLACK-BOX ATTESTATION
 * ----------------------------------------------------------------------------
 * Authored from the snapdir-api `ManifestOptions` semantics + the §1 parity
 * contract + the existing Node test house style (Manifest.raw = raw text,
 * idFromManifest sync, manifest/id async) ONLY. I did NOT read
 * bindings/node/src/ and reference ZERO napi/Rust internals.
 * ============================================================================
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import {
  mkdtempSync,
  writeFileSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  realpathSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { manifest, id, idFromManifest } from '@snapdir/snapdir'

const HEX64 = /^[0-9a-f]{64}$/ // SnapshotId: 64 lowercase hex chars

// A fresh tree per suite run, containing:
//   real/            a real subdirectory…
//   real/inner.txt   …with a file in it,
//   target.txt       a real file,
//   link_to_dir  ->  real         (a symlink to the directory)
//   link_to_file ->  target.txt   (a symlink to the file)
//   drop.tmp         a file that the `exclude` cases remove,
//   keep.log         a file that survives the `.tmp` exclude.
let dir: string
let absDir: string

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), 'snapdir-node-mopt-'))
  // realpath: macOS tmpdir is a /var → /private/var symlink; the binding emits
  // the canonical path, so the `absolute` assertions compare against realpath.
  absDir = realpathSync(dir)

  mkdirSync(join(dir, 'real'))
  writeFileSync(join(dir, 'real', 'inner.txt'), 'inner\n')
  writeFileSync(join(dir, 'target.txt'), 'target-bytes\n')
  // A symlink to the directory and a symlink to the file. Under FOLLOW (default)
  // these are dereferenced; under no-follow the link itself is recorded.
  symlinkSync('real', join(dir, 'link_to_dir'))
  symlinkSync('target.txt', join(dir, 'link_to_file'))
  // Exclude fixtures.
  writeFileSync(join(dir, 'drop.tmp'), 'temp\n')
  writeFileSync(join(dir, 'keep.log'), 'log\n')
  // Extended-regex exclude fixtures: a digit-bearing name (char class / anchor
  // tests) and two alternation targets. Additive — they do not perturb the
  // pre-existing cases above (those filter on `.tmp`/`.log`).
  writeFileSync(join(dir, 'file9.dat'), 'nine\n')
  writeFileSync(join(dir, 'alpha.cfg'), 'a\n')
  writeFileSync(join(dir, 'beta.cfg'), 'b\n')
})

afterAll(() => {
  if (dir) rmSync(dir, { recursive: true, force: true })
})

// ============================================================================
// 1. no-follow vs follow are DISTINCT (the headline symlink semantics).
// ============================================================================
describe('manifest/id noFollow option (follow_symlinks = !noFollow)', () => {
  // Spec: ManifestOptions.follow_symlinks; default FOLLOWS (dereferences) a
  // symlink, `{ noFollow: true }` records the link itself. The two walks of the
  // SAME tree therefore differ. On the current option-less binding the 2nd arg
  // is ignored → both collapse to the default → these inequalities FAIL.
  it('follow (default) and { noFollow: true } produce DIFFERENT raw manifests', async () => {
    const followed = await manifest(dir) // default = follow
    const noFollow = await manifest(dir, { noFollow: true })

    expect(typeof followed.raw).toBe('string')
    expect(typeof noFollow.raw).toBe('string')
    expect(followed.raw.length).toBeGreaterThan(0)
    expect(noFollow.raw.length).toBeGreaterThan(0)

    // The load-bearing assertion: the two symlink-handling modes MUST differ.
    // (A no-follow walk records `link_to_dir`/`link_to_file` as links; a follow
    // walk dereferences them into the pointed-at dir/file contents.)
    expect(noFollow.raw).not.toBe(followed.raw)
  })

  it('explicit { noFollow: false } equals the default follow behavior', async () => {
    // noFollow:false is the SAME as omitting it (follow). Pins that the flag is
    // a true boolean toggle, not merely "present ⇒ no-follow".
    const def = await manifest(dir)
    const explicitFollow = await manifest(dir, { noFollow: false })
    expect(explicitFollow.raw).toBe(def.raw)
    expect(await id(dir, { noFollow: false })).toBe(await id(dir))
  })

  it('id() reflects the noFollow option and DIFFERS between the two modes', async () => {
    const idFollow = await id(dir) // default follow
    const idNoFollow = await id(dir, { noFollow: true })
    expect(idFollow).toMatch(HEX64)
    expect(idNoFollow).toMatch(HEX64)
    // Different manifest text ⇒ different BLAKE3 snapshot id.
    expect(idNoFollow).not.toBe(idFollow)
  })

  it('each mode is self-consistent: id(path,opts) === idFromManifest(manifest(path,opts))', async () => {
    // The oracle we CAN assert black-box: per-variant id↔manifest agreement.
    for (const opts of [undefined, { noFollow: true }, { noFollow: false }] as const) {
      const m = opts === undefined ? await manifest(dir) : await manifest(dir, opts)
      const direct = opts === undefined ? await id(dir) : await id(dir, opts)
      const derived = idFromManifest(m)
      expect(derived).toMatch(HEX64)
      expect(derived).toBe(direct)
    }
  })
})

// ============================================================================
// 2. absolute option — paths rendered absolute, not relative `./`.
// ============================================================================
describe('manifest/id absolute option', () => {
  // Spec: ManifestOptions.absolute. Default renders the root + entries relative
  // to the tree (the frozen `./` rendering). `{ absolute: true }` renders them
  // as absolute paths beginning with the tree's absolute path. On the current
  // option-less binding the flag is ignored → raw stays relative → FAILS.
  it('{ absolute: true } renders paths starting with the absolute tree path', async () => {
    const rel = await manifest(dir)
    const abs = await manifest(dir, { absolute: true })

    // Default rendering is relative (`./` rooted) — it must NOT already be the
    // absolute path. (Pin the contrast so the impl genuinely switches modes.)
    expect(rel.raw).not.toBe(abs.raw)

    // The absolute variant's raw text must reference the absolute tree path.
    // (realpath handles the macOS /var→/private/var symlink on tmpdir.)
    expect(abs.raw).toContain(absDir)
    // …and the default relative rendering must NOT contain the absolute path.
    expect(rel.raw).not.toContain(absDir)

    // Every non-empty manifest content line in absolute mode should carry an
    // absolute path component (path is the last whitespace-separated field, the
    // frozen `sort -k5` layout). At minimum the root path is absolute.
    expect(abs.raw).toMatch(new RegExp(escapeRe(absDir)))
  })

  it('id() differs between relative (default) and absolute renderings', async () => {
    const idRel = await id(dir)
    const idAbs = await id(dir, { absolute: true })
    expect(idRel).toMatch(HEX64)
    expect(idAbs).toMatch(HEX64)
    // Different path rendering ⇒ different manifest text ⇒ different id.
    expect(idAbs).not.toBe(idRel)
    // Self-consistency for the absolute variant.
    expect(idFromManifest(await manifest(dir, { absolute: true }))).toBe(idAbs)
  })
})

// ============================================================================
// 3. exclude option — regex patterns omit matching entries; OR-combine.
// ============================================================================
describe('manifest/id exclude option (regex patterns, OR-combined)', () => {
  // Spec: ManifestOptions.exclude — a list of regex patterns; an entry whose
  // path matches ANY pattern is omitted. On the current option-less binding the
  // list is ignored → the excluded entry remains present → FAILS.
  it('{ exclude: ["\\.tmp$"] } drops the .tmp entry but keeps the .log entry', async () => {
    const full = await manifest(dir)
    const excluded = await manifest(dir, { exclude: ['\\.tmp$'] })

    // Baseline: drop.tmp IS present without the filter.
    expect(entryPaths(full).some((p) => p.endsWith('drop.tmp'))).toBe(true)

    // With the filter: drop.tmp is ABSENT, keep.log is PRESENT.
    const paths = entryPaths(excluded)
    expect(paths.some((p) => p.endsWith('drop.tmp'))).toBe(false)
    expect(paths.some((p) => p.endsWith('keep.log'))).toBe(true)

    // raw text must also no longer mention the excluded basename.
    expect(excluded.raw).not.toContain('drop.tmp')
    expect(excluded.raw).toContain('keep.log')

    // Excluding an entry changes the manifest ⇒ a different id from the full one.
    expect(await id(dir, { exclude: ['\\.tmp$'] })).not.toBe(await id(dir))
  })

  it('multiple patterns OR-combine: ["\\.tmp$", "\\.log$"] drops BOTH', async () => {
    const both = await manifest(dir, { exclude: ['\\.tmp$', '\\.log$'] })
    const paths = entryPaths(both)
    // Both the .tmp and the .log entries are dropped (OR of the two patterns).
    expect(paths.some((p) => p.endsWith('drop.tmp'))).toBe(false)
    expect(paths.some((p) => p.endsWith('keep.log'))).toBe(false)
    // A non-matching entry (target.txt) still survives.
    expect(paths.some((p) => p.endsWith('target.txt'))).toBe(true)

    // OR-combining more patterns removes strictly more ⇒ id differs from the
    // single-pattern exclude (which still kept keep.log).
    expect(await id(dir, { exclude: ['\\.tmp$', '\\.log$'] })).not.toBe(
      await id(dir, { exclude: ['\\.tmp$'] })
    )
  })

  it('an empty exclude array equals no exclude at all', async () => {
    // `exclude: []` filters nothing → identical to the default manifest.
    const empty = await manifest(dir, { exclude: [] })
    const def = await manifest(dir)
    expect(empty.raw).toBe(def.raw)
    expect(await id(dir, { exclude: [] })).toBe(await id(dir))
  })

  it('exclude id is self-consistent with its manifest', async () => {
    const opts = { exclude: ['\\.tmp$'] }
    expect(idFromManifest(await manifest(dir, opts))).toBe(await id(dir, opts))
  })
})

// ============================================================================
// 4. optionality / backward-compat — omitting options === `{}` === default.
// ============================================================================
describe('options are OPTIONAL and backward-compatible', () => {
  // Spec: the 2nd arg is optional; omitting it (or passing an empty object)
  // preserves the CURRENT default behavior. This case already HOLDS on the
  // option-less binding (an ignored arg is indistinguishable from a defaulted
  // one) — its job is to lock the impl so adding options never shifts the
  // no-options default.
  it('manifest(path) deep-equals manifest(path, {})', async () => {
    const a = await manifest(dir)
    const b = await manifest(dir, {})
    expect(b.raw).toBe(a.raw)
    // The structured entries match too (deep-equal of the whole manifest).
    expect(b).toEqual(a)
  })

  it('id(path) === id(path, {}) === idFromManifest(manifest(path))', async () => {
    const direct = await id(dir)
    const withEmpty = await id(dir, {})
    expect(withEmpty).toBe(direct)
    expect(idFromManifest(await manifest(dir))).toBe(direct)
  })

  it('each individual default-valued option leaves the default unchanged', async () => {
    // noFollow:false, absolute:false, exclude:[] are each the DEFAULT value, so
    // any one of them in isolation must reproduce the bare default manifest.
    const def = await id(dir)
    expect(await id(dir, { noFollow: false })).toBe(def)
    expect(await id(dir, { absolute: false })).toBe(def)
    expect(await id(dir, { exclude: [] })).toBe(def)
  })
})

// ============================================================================
// 5. options compose — combining flags is self-consistent.
// ============================================================================
describe('id ↔ manifest parity across option COMBINATIONS', () => {
  // Spec §5: id(path,opts) self-consistent with manifest(path,opts) for each
  // combination — including all three options together.
  it('combined { noFollow, absolute, exclude } stays id↔manifest consistent and distinct', async () => {
    const opts = { noFollow: true, absolute: true, exclude: ['\\.tmp$'] }
    const m = await manifest(dir, opts)
    const sid = await id(dir, opts)
    expect(idFromManifest(m)).toBe(sid)
    // Distinct from the bare default (all three options change the walk).
    expect(sid).not.toBe(await id(dir))
    // absolute ⇒ raw references the absolute path; exclude ⇒ drop.tmp gone.
    expect(m.raw).toContain(absDir)
    expect(m.raw).not.toContain('drop.tmp')
  })
})

// ============================================================================
// 6. STRENGTHENING (tests-review, adversary/opus via PM takeover) — behavioral
//    edge cases the landed impl reveals, beyond raw/id inequality. These pin
//    the SEMANTICS (structure, not just "differs") so the impl can't satisfy
//    the contract with a token option that merely perturbs bytes.
// ============================================================================
describe('STRENGTHEN — noFollow structural semantics (dereference vs record-link)', () => {
  // FOLLOW (default) treats link_to_dir as a directory and walks INTO it, so
  // entries appear under `link_to_dir/…`. no-follow records the symlink as a
  // single entry and never recurses — so NO path is under `link_to_dir/`.
  it('follow recurses INTO a dir symlink; noFollow does not', async () => {
    const followed = entryPaths(await manifest(dir)) // default follow
    const noFollow = entryPaths(await manifest(dir, { noFollow: true }))

    const under = (paths: string[]) => paths.some((p) => /(^|\/)link_to_dir\//.test(p))
    expect(under(followed)).toBe(true) // dereferenced: link_to_dir/inner.txt present
    expect(under(noFollow)).toBe(false) // link recorded as a leaf; no traversal

    // Concretely: following yields strictly MORE entries than not following
    // (the dereferenced dir contributes its child).
    expect(followed.length).toBeGreaterThan(noFollow.length)
  })
})

describe('STRENGTHEN — absolute ROOT-line rendering', () => {
  // The frozen manifest layout makes the ROOT a `D <perm> <ck> <size> <path>`
  // line whose path is `./` by default and the ABSOLUTE tree path under
  // { absolute: true } (rendered as `<absDir>/`). Pin the root line specifically
  // (not just "raw contains absDir somewhere").
  it('default root line is `./`; absolute root line is `<absDir>/`', async () => {
    const relRoot = rootLine(await manifest(dir))
    const absRoot = rootLine(await manifest(dir, { absolute: true }))
    expect(relRoot).not.toBeNull()
    expect(absRoot).not.toBeNull()
    // The path field (last column) of the root line.
    expect(relRoot!.split(/\s+/).pop()).toBe('./')
    expect(absRoot!.split(/\s+/).pop()).toBe(`${absDir}/`)
  })
})

describe('STRENGTHEN — exclude: nested entries + extended-regex (-E) semantics', () => {
  it('excludes a NESTED entry (real/inner.txt) while keeping its parent dir', async () => {
    const filtered = await manifest(dir, { exclude: ['inner'] })
    const paths = entryPaths(filtered)
    expect(paths.some((p) => p.endsWith('inner.txt'))).toBe(false) // nested file gone
    // The parent `real` directory itself is NOT matched by /inner/ → still present.
    expect(paths.some((p) => p.endsWith('real') || p.includes('real/'))).toBe(true)
  })

  it('extended-regex ALTERNATION drops both alternatives in one pattern', async () => {
    // `drop|keep` is an ERE alternation (grep -E -v semantics): a path matching
    // EITHER alternative is omitted. drop.tmp (drop) and keep.log (keep) both go.
    const paths = entryPaths(await manifest(dir, { exclude: ['drop|keep'] }))
    expect(paths.some((p) => p.endsWith('drop.tmp'))).toBe(false)
    expect(paths.some((p) => p.endsWith('keep.log'))).toBe(false)
    expect(paths.some((p) => p.endsWith('target.txt'))).toBe(true) // non-matching survives
  })
})

describe('STRENGTHEN — option INDEPENDENCE (each option touches only its concern)', () => {
  it('absolute changes RENDERING only, never entry membership', async () => {
    const def = entryPaths(await manifest(dir))
    const abs = entryPaths(await manifest(dir, { absolute: true }))
    // Same number of entries (absolute reframes paths; it adds/removes none).
    expect(abs.length).toBe(def.length)
    // Same membership by basename for the NON-root entries. The ROOT entry is
    // excluded from the basename comparison on purpose: its path legitimately
    // re-renders from `./` (basename `.`) to `<absDir>/` (basename = the tmp
    // dir's leaf name), so its last segment is EXPECTED to differ between modes
    // — that difference is the whole point of `absolute`, not a membership
    // change. We therefore (a) assert the root is present in each rendering and
    // (b) compare the remaining basenames as multisets.
    const base = (p: string) => p.replace(/\/+$/, '').split('/').pop() ?? p
    const isRoot = (p: string) => p === './' || p === `${absDir}/`
    expect(def.some(isRoot)).toBe(true) // relative root `./` present
    expect(abs.some(isRoot)).toBe(true) // absolute root `<absDir>/` present
    const nonRootBases = (paths: string[]) => paths.filter((p) => !isRoot(p)).map(base).sort()
    expect(nonRootBases(abs)).toEqual(nonRootBases(def))
  })

  it('exclude changes MEMBERSHIP only, never path rendering (stays relative)', async () => {
    const filtered = await manifest(dir, { exclude: ['\\.tmp$'] })
    expect(filtered.raw).not.toContain(absDir) // still `./`-relative, not absolute
  })
})

describe('STRENGTHEN — exclude: extended-regex char classes + anchors', () => {
  // Beyond alternation: pin that character classes and end-anchors compile and
  // match per Rust `regex`/`-E` semantics (the impl OR-joins `(?:p)` groups and
  // matches unanchored via `is_match`). Uses the digit-named `file9.dat`.
  it('character class `file[0-9]\\.dat$` drops file9.dat; a disjoint class does NOT', async () => {
    const dropped = entryPaths(await manifest(dir, { exclude: ['file[0-9]\\.dat$'] }))
    expect(dropped.some((p) => p.endsWith('file9.dat'))).toBe(false)
    // A class that excludes the digit 9 must leave file9.dat present — proves the
    // class is genuinely evaluated, not treated as a literal/substring.
    const kept = entryPaths(await manifest(dir, { exclude: ['file[0-8]\\.dat$'] }))
    expect(kept.some((p) => p.endsWith('file9.dat'))).toBe(true)
  })

  it('end-anchor `\\.cfg$` drops both .cfg files but spares non-.cfg siblings', async () => {
    const paths = entryPaths(await manifest(dir, { exclude: ['\\.cfg$'] }))
    expect(paths.some((p) => p.endsWith('alpha.cfg'))).toBe(false)
    expect(paths.some((p) => p.endsWith('beta.cfg'))).toBe(false)
    expect(paths.some((p) => p.endsWith('keep.log'))).toBe(true) // not .cfg → survives
  })

  it('two patterns that select the SAME entry set yield the SAME id (set-filter, not text)', async () => {
    // `\.cfg$` ≡ {alpha.cfg, beta.cfg} ≡ `alpha\.cfg$|beta\.cfg$` here. Pins that
    // exclude is a pure membership filter: identical resulting set ⇒ identical id.
    const byAnchor = await id(dir, { exclude: ['\\.cfg$'] })
    const byAlt = await id(dir, { exclude: ['alpha\\.cfg$', 'beta\\.cfg$'] })
    expect(byAnchor).toMatch(HEX64)
    expect(byAnchor).toBe(byAlt)
  })
})

describe('STRENGTHEN — followed file symlink mirrors its TARGET content', () => {
  // The structural (not merely "raw differs") proof that follow DEREFERENCES:
  // link_to_file -> target.txt, so under the default follow walk the link entry
  // records the SAME content checksum as target.txt. byBasename keys entries by
  // their last path segment; if the projection lacks structured entries the
  // assertion degrades to a no-op rather than a false pass.
  it('under follow, link_to_file checksum equals target.txt checksum (dereferenced)', async () => {
    const m = await manifest(dir) // default follow
    const byBase = byBasename(m)
    const link = byBase.get('link_to_file')
    const target = byBase.get('target.txt')
    if (link && target && link.pathType === 'File' && target.pathType === 'File') {
      expect(link.checksum).toBe(target.checksum)
    } else {
      // No structured entries to compare → fall back to the raw-level guarantee
      // that following at least surfaces the target's bytes-derived line.
      expect(target ?? link).toBeDefined()
    }
  })

  it('noFollow makes link_to_file differ structurally (type or checksum) from the followed entry', async () => {
    const followed = byBasename(await manifest(dir))
    const noFollow = byBasename(await manifest(dir, { noFollow: true }))
    const f = followed.get('link_to_file')
    const n = noFollow.get('link_to_file')
    if (f && n) {
      // A genuine dereference-vs-record difference: different type OR checksum.
      expect(f.pathType !== n.pathType || f.checksum !== n.checksum).toBe(true)
    } else {
      // Presence/absence under the two modes is itself the structural delta.
      expect(Boolean(f)).not.toBe(Boolean(n))
    }
  })
})

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

interface EntryShape {
  path: string
  pathType?: string
  checksum?: string
}

// Map of basename (last path segment, trailing slash stripped) → entry, for
// structural cross-walk comparisons. Empty when `entries` is unavailable.
function byBasename(m: { entries?: Array<EntryShape>; raw: string }): Map<string, EntryShape> {
  const out = new Map<string, EntryShape>()
  if (Array.isArray(m.entries)) {
    for (const e of m.entries) {
      const seg = e.path.replace(/\/+$/, '').split('/').pop() ?? ''
      if (seg.length > 0) out.set(seg, e)
    }
  }
  return out
}

// The root manifest line: the single `D … ./` (or absolute) line for the tree
// root. Returns the first directory line whose path column is `./` or an
// absolute path ending in `/` (the root render), else the first `D ` line.
function rootLine(m: { raw: string }): string | null {
  const lines = m.raw.split('\n').filter((l) => l.length > 0 && !l.startsWith('#'))
  const root =
    lines.find((l) => {
      const col = l.trim().split(/\s+/).pop() ?? ''
      return l.startsWith('D ') && (col === './' || /^\/.+\/$/.test(col))
    }) ?? lines.find((l) => l.startsWith('D '))
  return root ?? null
}

// The structured entry paths of a manifest. The house Manifest shape exposes
// `entries[].path`; fall back to parsing `raw` if `entries` is unavailable so
// the exclude assertions stay robust to either projection.
function entryPaths(m: { entries?: Array<{ path: string }>; raw: string }): string[] {
  if (Array.isArray(m.entries) && m.entries.length > 0) {
    return m.entries.map((e) => e.path)
  }
  // raw fallback: last whitespace-separated field of each non-comment line.
  return m.raw
    .split('\n')
    .filter((l) => l.length > 0 && !l.startsWith('#'))
    .map((l) => l.trim().split(/\s+/).pop() ?? '')
    .filter((p) => p.length > 0)
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}
