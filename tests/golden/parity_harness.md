# parity_harness.md — BLACK-BOX SPEC for `tests/golden/run_parity.sh` + the per-binding DRIVER interface

**Gate:** `parity-harness-spec-tests` (Phase 36, owner `adversary`, opus) — the SPEC leg of the
`parity-harness-{spec-tests,impl,tests-review}` triple. This is the **non-negotiable correctness
contract that EVERY binding (Node / Python / Go / C / C++ / Zig / Java) is measured against**.

**Authoring discipline (black-box).** This document specifies the **contract** the harness and
every driver must satisfy — derived from the LOCKED design (`.gatesmith/reviews/fixtures-corpus.md`,
D1-D4), the frozen oracle baseline (`tests/golden/gen_fixtures.sh` + `tests/golden/expected/*`), and
the oracle CLI behavior (`crates/snapdir-api/tests/m0_golden_parity.rs` + the `snapdir-cli` trycmd
`--help` captures for `manifest`/`id`/`push`/`fetch`/`pull`/`checkout`). It does **NOT** prescribe a
binding's internal implementation (that is Phases 37-42) — it specs the INTERFACE they plug into.

**Deliverable of the `-impl` gate:** `tests/golden/run_parity.sh` + `tests/golden/drivers/rust.sh`,
proven **green with the oracle reference driver** across `fixtures × file://` (+ the per-PR network
legs when sidecars are up). **Expected-fail rationale:** `tests/golden/run_parity.sh` does **not exist
yet**, so this contract is unimplemented today — that is correct for an authoring gate.

---

## 0. The reference driver = the oracle itself (the impl gate's green bar)

The harness is **driver-agnostic**. The driver is selected by env `PARITY_DRIVER` and defaults to the
**reference driver** `tests/golden/drivers/rust.sh`, which is a thin wrapper over the workspace
`snapdir` binary (the pinned 1.10.0 oracle, the same bin `gen_fixtures.sh` and the m0 suite use). The
non-negotiable green bar at the `-impl` gate is: **`run_parity.sh` passes with the reference driver
across every fixture on `file://`** (the oracle compared against itself + the frozen `expected/*`).
Each Phase-37-42 binding later ships `tests/golden/drivers/<lang>.sh` (or a binary) plugged into the
SAME harness via `PARITY_DRIVER=tests/golden/drivers/<lang>.sh`.

> A driver that merely re-shells the oracle is the trivial reference. A binding's driver MUST call the
> binding's own code (napi / PyO3 / C-ABI / …) — never the oracle. The harness cannot tell them apart;
> it only checks byte/hex parity against the frozen `expected/*` and round-trip self-consistency.

---

## 1. THE DRIVER INTERFACE (the contract every binding implements)

A driver is an executable invoked as `"$PARITY_DRIVER" <subcommand> <args…>`. It is a small,
language-agnostic subprocess protocol. Every driver MUST implement these subcommands with **exactly**
these semantics. All stdout is **byte-exact** (no banners, no progress, no trailing decoration beyond
what is specified). Diagnostics go to **stderr only**. Exit code `0` = success, non-zero = failure.

The harness ALWAYS invokes a driver with the environment scrubbed of store-routing leakage (see §1.6)
so that flags/args are the sole source of truth.

### 1.1 `<driver> manifest <path> [--no-follow] [--absolute] [--exclude <RE>]…`

- Emits the manifest TEXT to **stdout**, **byte-identical** to `snapdir manifest [flags] <path>`.
- The trailing `\n` IS part of the contract (matches `gen_fixtures.sh` `printf '%s\n'` capture and the
  m0 suite's "trailing newline is part of the byte contract").
- Flags map 1:1 to the oracle `manifest` flags:
  - `--no-follow` → oracle `--no-follow` (plain `find`, records links not targets).
  - `--absolute`  → oracle `--absolute` (absolute PATH column; root renders as `<dir>/`).
  - `--exclude <RE>` → oracle `--exclude <RE>`; **repeatable** (OR-combined, mirroring
    `combine_excludes`). The driver MUST accept multiple `--exclude` occurrences.
- The root line is ALWAYS a `D ` line: `D <octal-perm> <hex-checksum> <size> ./` (or the absolute
  root under `--absolute`). Entry lines are `D`/`F` `TYPE PERM CHECKSUM SIZE PATH`, sorted by the
  oracle's `sort -k5` path ordering (`LC_ALL=C`).
- Checksum column defaults to BLAKE3. (`--checksum-bin` is OUT of the parity driver's required
  surface — the corpus `expected/*` are all default BLAKE3; a binding MAY implement it but the
  harness does not exercise alternate checksum bins. The m0 Rust suite already pins
  `--checksum-bin` parity at the API layer.)
- Exit `0` and emit the manifest on success; non-zero + a stderr message on any walk error.

### 1.2 `<driver> id <path> [--no-follow] [--absolute] [--exclude <RE>]…`

- Emits the 64-char **lowercase hex** snapshot id to **stdout**, followed by a single `\n` (matches
  `expected/<F>.id` = `printf '%s\n'`).
- The id is **ALWAYS BLAKE3 of the manifest TEXT** that `manifest` (with the SAME flags) would emit —
  regardless of any checksum-column choice. This is the m0 invariant
  (`id == BLAKE3(manifest_text)`, `snapdir id` has no `--checksum-bin`). A driver MUST therefore
  satisfy the **self-consistency** law: `id <path> [flags]` == BLAKE3(`manifest <path> [flags]`)
  == `expected/<F>.id`.
- The same option flags as `manifest` apply (the option-variant id is the id of the option-rendered
  manifest — exactly how the oracle obtains it: `snapdir manifest [flags] <path> | snapdir id`).
- Exit `0` + the 64-hex line on success; non-zero on error.

### 1.3 `<driver> push <path> <store_uri> [--jobs N]`  (network/file backends)

- Pushes the tree at `<path>` to `<store_uri>` and emits the resulting **snapshot id** (64-hex `\n`)
  to **stdout** — identical to `snapdir push --store <store_uri> <path>` stdout (the m0 `capture`
  helper relies on `push` stdout being exactly the 64-hex id).
- `<store_uri>` is one of the §3 backend URIs (`file://`, `s3://`, `b2://`, `ssh://`, `sftp://`,
  `gs://`). The driver maps it to the oracle `--store <uri>` (or the binding's equivalent).
- Optional trailing tuning args (`--jobs N`, etc.) are accepted and may be ignored; they MUST NOT
  change the produced id.
- The pushed id MUST equal `id <path>` for the same tree (push does not mutate the manifest).
- Exit `0` + id on success; non-zero on transport failure.

### 1.4 `<driver> fetch <id> <store_uri>`  (network/file backends)

- Fetches snapshot `<id>` from `<store_uri>` into the local cache (objects + manifest), mirroring
  `snapdir fetch --store <store_uri> --id <id>`. No tree materialization. stdout is unspecified
  (diagnostics to stderr); exit `0` iff the snapshot's objects + manifest are all retrievable and
  BLAKE3-verify. Non-zero if any object/manifest is missing or fails verification.

### 1.5 `<driver> checkout <id> <store_uri> <dest>`  (network/file backends)

- Materializes snapshot `<id>` from `<store_uri>` into directory `<dest>`, mirroring
  `snapdir pull --store <store_uri> --id <id> <dest>` (a fetch-then-checkout; equivalently the
  oracle `checkout <dest>` after a `fetch`). On success the tree at `<dest>` re-manifests to `<id>`
  (see §2.3). Exit `0` on success; non-zero on transport/materialization failure.
- The reference driver MAY implement this as `fetch` + `snapdir checkout --store <uri> --id <id>
  <dest>`, or directly as `snapdir pull`. Either is acceptable so long as `dest` re-manifests to
  `<id>` byte-for-byte.

### 1.6 Environment the harness sets / scrubs for EVERY driver invocation

**Scrubbed (always removed before invoking a driver — mirrors `gen_fixtures.sh` + m0):**
`SNAPDIR_STORE`, `SNAPDIR_OBJECTS_STORE`, `SNAPDIR_MANIFEST_CONTEXT`. The store is passed ONLY as the
explicit `<store_uri>` arg, never via env, so a leaked env store cannot mask a routing bug.

**Set (deterministic baseline):**
- `LC_ALL=C` — pins `sort -k5` collation identical to the generator.
- `SNAPDIR_CACHE_DIR` — a fresh per-run scratch cache dir (isolates fetch/checkout; wiped per run).
- `SNAPDIR_CATALOG_DB_PATH` — a fresh per-run sqlite catalog path (so catalog writes are hermetic
  and never touch a developer's catalog).
- `SNAPDIR_NO_PROGRESS=1` (and the driver SHOULD pass `--quiet`/`--no-progress` to the oracle) so no
  progress/banner bytes contaminate stdout.

**Network-backend sidecar env (set only for the network legs, per fixtures-corpus.md §3 + the
`snapdir-stores` test env names):**
- **s3 / b2 (minio @ `http://127.0.0.1:9000`):** `SNAPDIR_S3_TEST_ENDPOINT=http://127.0.0.1:9000`
  (+ `SNAPDIR_B2_TEST_ENDPOINT` for the b2 leg), path-style auto-on, the minio access/secret creds
  the sidecar provisions. `s3://` and `b2://` route through `S3Store` against minio.
- **ssh / sftp (sshd @ `127.0.0.1:2222`, user `snapdir-parity`):** key
  `/workspace/scripts/sidecar_ssh_key`, `known_hosts` `…/sidecar_known_hosts` (the harness wires
  these into the store config / URI per the corpus §3 row).
- **gs (LIVE GCS, creds-gated nightly only — D1):** the project/bucket/account from the GCS
  integration creds. **NOT minio.** Skipped-not-failed when the creds env is absent (§3.4).

Each backend uses an **isolated bucket/prefix (or remote path) per fixture run** so concurrent/repeat
runs never collide; minio's data dir is wiped by `sidecars-down.sh`.

---

## 2. THE PARITY ASSERTIONS (per fixture × per applicable backend)

Let `F` range over the 8 fixtures (`symlinks` contributing TWO captures: `symlinks-follow` and
`symlinks-nofollow`), and `B` over the applicable backends for the current schedule (§3). The
`expected/*` files are the **frozen oracle truth** (locked by `golden-fixtures-freeze`).

### 2.1 manifest parity (BYTE-FOR-BYTE) — every fixture, ALWAYS (the `file://`/local leg)

For each fixture `F`:
- `driver manifest <workdir>/<F>` **==** `tests/golden/expected/<F>.manifest`, compared **byte-for-byte**
  (e.g. `cmp -s` / `diff` on the exact bytes — NOT a normalized, trimmed, sorted, or subset compare).
  A single differing byte (perm column, checksum, size, a path-encoding divergence, a missing/extra
  trailing newline, line ordering) is a **FAIL**.
- The `--no-follow` variant: `driver manifest <workdir>/symlinks --no-follow` **==**
  `expected/symlinks-nofollow.manifest`, byte-for-byte; and the default (follow) `driver manifest
  <workdir>/symlinks` **==** `expected/symlinks-follow.manifest`.
- `unicode-paths` is the single highest-value cross-binding case: its PATH column MUST survive
  byte-for-byte (NFC vs NFD distinct, RTL, emoji, line-separator, leading dot/dash, mixed-case
  collation, digit-vs-letter ordering) — a binding's path-encoding bug surfaces here.
- `permissions` MUST render the octal PERM column byte-identically, **including the setuid/setgid/
  sticky high bits exactly as the frozen manifest recorded them** (honoring the D4 setuid-fallback
  baked into `expected/permissions.manifest`).

### 2.2 id parity (64-hex exact + self-consistency) — every fixture, ALWAYS

For each fixture `F`:
- `driver id <workdir>/<F>` **==** `tests/golden/expected/<F>.id` (64 lowercase hex, exact).
- **Self-consistency:** `driver id <workdir>/<F>` **==** `BLAKE3(driver manifest <workdir>/<F>)` —
  i.e. the id the driver reports is the BLAKE3 of the exact manifest text the driver emits. The
  harness verifies this by piping the driver's manifest through the **oracle** `snapdir id` and
  asserting equality (so a binding cannot pass `id` by hardcoding the expected hex while emitting a
  divergent manifest). The `--no-follow` id likewise == `expected/symlinks-nofollow.id`.

### 2.3 round-trip (network + file backends) — push → fetch → checkout re-manifest equality

For each applicable backend `B` and each fixture in the schedule's round-trip set:
1. `id_local := driver id <workdir>/<F>`  (and assert `id_local == expected/<F>.id`).
2. `id_push  := driver push <workdir>/<F> <store_uri(B,F)>`.
   - **Assert `id_push == id_local`** (push must not perturb the manifest/id).
3. `driver fetch <id_push> <store_uri(B,F)>` exits `0` (all objects + the manifest retrievable and
   BLAKE3-verifying through the transport).
4. `driver checkout <id_push> <store_uri(B,F)> <dest>` materializes the tree, then
   `id_dest := oracle id <dest>` (re-manifest the materialized tree with the **reference oracle**, so
   the round-trip is judged by an independent re-walk).
   - **Assert `id_dest == id_push == expected/<F>.id`** — byte-for-byte reproduction THROUGH the store.
- This proves transport fidelity: identical manifest + id survive a real push/fetch/checkout cycle.
- `file://` MAY also round-trip (cheap; the reference driver SHOULD exercise it as a self-check that
  the round-trip path itself is correct even with no sidecar).

> Fixtures whose checkout cannot be losslessly re-walked (e.g. the `symlinks` fixture's **dangling**
> `broken` and **escaping** `escape` links, or setuid bits a CI mount strips) are exercised for
> manifest/id parity (§2.1/§2.2) but MAY be excluded from the round-trip set when the destination
> filesystem cannot reproduce them; the harness MUST document any such exclusion as a **SKIP line**
> (never a silent drop), and `large-tree` round-trips are nightly-only (§3).

---

## 3. THE BACKEND MATRIX + PR / NIGHTLY GATING (per fixtures-corpus.md D1/D3)

The harness reads the toggle **`SNAPDIR_PARITY_NIGHTLY`** (`1` = include the heavy/nightly legs;
unset/`0` = the fast per-PR sweep only). A leg = one `(driver × backend × fixture-class)` cell.

### 3.1 `file://` — ALWAYS, offline, the hard per-PR gate
- `file://` × **all 8 fixtures** (incl. both symlink captures), manifest + id parity for the Rust
  driver AND the binding-under-test driver. No sidecars. Runs first / standalone.
- Per-PR `file://` wall-clock budget: **≤ 90 s** full sweep (8 fixtures × Rust + one binding driver);
  the Rust-oracle `file://` leg alone **≤ 45 s** (D2). `large-tree` dominates this budget.

### 3.2 Per-PR network legs (small fixtures only) — when sidecars are up
- **`s3://`** (minio, `S3Store`) and **`sftp://`** (sshd, pure-SFTP) — the two cheapest, highest-
  coverage transports (one S3-API + one pure-SFTP).
- Round-trip (§2.3) over the **small fixtures**: `empty`, `single-file`, `nested`, `unicode-paths`,
  `symlinks` (follow), `identical-content`, `permissions`. (NOT `large-tree`.)
- Skipped-not-failed when the sidecars/creds are unavailable (§3.4).

### 3.3 Nightly-only legs (`SNAPDIR_PARITY_NIGHTLY=1`)
- **`b2://`** (minio, S3-on-minio — redundant transport variant) and **`ssh://`** (sshd, shares the
  daemon with sftp) across **all** fixtures (round-trip).
- **`large-tree` × ALL network backends** (`s3`, `b2`, `sftp`, `ssh`, and the live `gs` leg) — the
  heavy cell deferred off the 90 s PR budget.
- **Live `gs://`** (D1): round-trip against a **real GCS bucket** (native Google Cloud SDK, NOT
  minio) using the GCS integration creds. Skipped-not-failed when those creds are absent (§3.4).
- **Budget-overflow rule:** if any per-PR leg exceeds the 90 s budget in CI, it MOVES to nightly (the
  leg is one toggleable cell).

### 3.4 SKIP rules (a SKIP is NOT a failure, but MUST be reported — no silent drop)
- A network backend whose **sidecar is down** (minio/sshd not listening — `sidecars-health.sh`
  reports unhealthy) → emit a `SKIP <fixture> <backend> — sidecar unavailable` line, **exit-0 for
  that cell**, do NOT fail the run.
- The live `gs://` leg with **no GCS creds in env** → `SKIP <fixture> gs — GCS creds absent`,
  exit-0 for that cell.
- A nightly cell when `SNAPDIR_PARITY_NIGHTLY` is unset → not run; the summary states it was deferred
  (a deferral line, not counted as a failure).
- **CRITICAL:** a SKIP is reported in BOTH the per-cell output AND the final summary count
  (`SKIPPED=<n>`). The run NEVER silently omits a backend — an un-reported missing leg is itself a
  harness bug. SKIP ≠ PASS in the summary; it is its own tally.

---

## 4. HARNESS OUTPUT CONTRACT (`run_parity.sh`)

- For **each `(fixture, backend, assertion)` cell** the harness prints exactly ONE status line:
  - `PASS  <fixture> <backend> <assertion>`  (e.g. `PASS unicode-paths file manifest`)
  - `FAIL  <fixture> <backend> <assertion> — <reason>` (and, on a byte mismatch, a short diff to
    stderr; the cell is counted as a failure)
  - `SKIP  <fixture> <backend> <assertion> — <reason>` (per §3.4)
- A final **summary line**: `SUMMARY driver=<PARITY_DRIVER> PASSED=<p> FAILED=<f> SKIPPED=<s>`
  (+ a nightly/deferred note when applicable).
- **Exit code:** `0` **iff FAILED == 0** (skips allowed and tallied); **non-zero** on ANY manifest /
  id / round-trip mismatch (FAILED ≥ 1). Exit non-zero is also required if the harness cannot even
  locate the driver or a required `expected/*` file (a missing baseline is a hard error, not a skip).
- **Driver selection:** the harness reads `PARITY_DRIVER` (a path or command); when unset it defaults
  to `tests/golden/drivers/rust.sh` (the oracle reference driver). The harness MUST run identically
  for any driver — the only difference is which executable produces the manifest/id/round-trip output.
- **Sidecar lifecycle (network legs only):** the harness runs `sidecars-up.sh && sidecars-health.sh`
  before the network legs and `sidecars-down.sh` after (always exit-0; cleans `/tmp/minio-data`). If
  `sidecars-health.sh` reports unhealthy, the network legs SKIP (§3.4) rather than fail. The `file://`
  sweep needs no sidecars and runs without them.

---

## 5. DETERMINISM + FROZEN-BASELINE NOTE (hermetic)

- The `expected/*.manifest` + `*.id` are the **frozen oracle truth** (locked at
  `golden-fixtures-freeze` into `.gatesmith/golden-fixtures.sha.lock`).
- The harness **regenerates the fixtures via `gen_fixtures.sh` first** (deterministic: fixed names,
  fixed seed-derived contents, `LC_ALL=C` sort, NO `$RANDOM`/dates/mtimes), so a run is fully
  hermetic — it compares a freshly-materialized tree against the frozen baseline. Re-running yields
  identical results.
- A driver MUST NOT introduce non-determinism (no timestamps, no random ordering); the manifest
  format records only path/type/perm/checksum/size (no mtimes), so a deterministic tree → a
  byte-stable manifest → a stable id.
- The D4 setuid-fallback is already baked into `gen_fixtures.sh` and `expected/permissions.manifest`:
  if the mount strips setuid/setgid, the generator records the high-bit-stripped variant for ONLY
  those two lines, so the frozen baseline stays consistent with what a driver re-walks on the same
  mount. The harness MUST regenerate (not assume) before comparing so the baseline and the live tree
  agree on that mount.

---

## 6. EXPECTED-FAIL RATIONALE (authoring-gate truth)

- There is **no `tests/golden/run_parity.sh` and no `tests/golden/drivers/rust.sh` yet** — this
  contract is **unimplemented**, which is the correct state for a `-spec-tests` authoring gate.
- The `-impl` gate (sonnet) writes `run_parity.sh` + `drivers/rust.sh` against THIS spec and proves
  the green bar: **the reference (oracle) driver passes byte/hex/round-trip across the fixtures on
  `file://`** (+ the per-PR network legs when sidecars are up). The `-tests-review` gate (adversary)
  then audits the landed harness against this spec for any weakened assertion (e.g. a normalized
  compare slipped in where byte-for-byte was required, a silently dropped backend, a skip that should
  have been a fail).
- A staged reference sketch of the compare loop is provided alongside this file as
  `.gatesmith/pending-tests/run_parity.ref.sh` (pseudo/real bash) — illustrative, NOT the harness;
  the gate check is `test -f .gatesmith/pending-tests/parity_harness.md`.

---

## Appendix A — the fixture × assertion × schedule matrix (quick reference)

| Fixture | manifest+id (file, per-PR) | round-trip s3+sftp (per-PR, small) | round-trip b2+ssh (nightly) | round-trip gs live (nightly+creds) | large-tree×net (nightly) |
|---|---|---|---|---|---|
| empty | ✅ | ✅ | ✅ | ✅ | — |
| single-file | ✅ | ✅ | ✅ | ✅ | — |
| nested | ✅ | ✅ | ✅ | ✅ | — |
| unicode-paths | ✅ | ✅ | ✅ | ✅ | — |
| symlinks-follow | ✅ | ✅ (dangling/escape may SKIP round-trip) | ✅ | ✅ | — |
| symlinks-nofollow | ✅ (`--no-follow`) | n/a (manifest/id only) | n/a | n/a | — |
| identical-content | ✅ | ✅ | ✅ | ✅ | — |
| permissions | ✅ (high bits per D4) | ✅ (setuid may SKIP round-trip) | ✅ | ✅ | — |
| large-tree | ✅ (dominates 90 s budget) | — (nightly only) | ✅ | ✅ | ✅ all net backends |

Legend: ✅ = asserted in that schedule; — = not in that cell; SKIP/deferred lines are still printed
and tallied (§3.4, §4).

---

*Authored by the `adversary` teammate for `parity-harness-spec-tests` (Phase 36, black-box). No
code/fixtures produced or modified; this is the contract the `-impl` gate implements as
`tests/golden/run_parity.sh` + `tests/golden/drivers/rust.sh`.*
