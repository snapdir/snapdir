"""Black-box contract spec for ``snapdir size`` in the Python (pyo3/maturin) binding (Phase 46).

Authored from the SPEC + the binding's PUBLIC surface ONLY. No implementation
visibility: this file was written WITHOUT reading ``crates/snapdir-ffi/src/`` or
``crates/snapdir-api/src/``. It reads only the public ``snapdir.pyi`` stub and
mirrors the conventions of ``bindings/python/tests/python_api.py``.

The three symbols under test do NOT exist yet, so this file is EXPECTED to error
at collection (``AttributeError``) until the ``python-size-impl`` gate lands:
  * ``snapdir.SizeStats``     — frozen stats object; int attrs.
  * ``snapdir.manifest_size`` — SYNC, pure: Manifest -> SizeStats.
  * ``snapdir.size``          — async: (StoreUri, SnapshotId | None) -> SizeStats.

Test harness contract (mirrors python_api.py): pytest + pytest-asyncio with
``asyncio_mode = "auto"`` so plain ``async def test_*`` run without an explicit
``@pytest.mark.asyncio`` marker.

Semantics pinned (objects stored UNCOMPRESSED):
  logical_bytes  = Σ size over ALL File entries (duplicates counted)
  physical_bytes = Σ size over UNIQUE checksums  == on-disk ``.objects/`` byte total
  files          = count of File entries
  objects        = count of distinct checksums

Deterministic fixture (parity with cpp/go/java/node/Rust):
  seed/a/x.txt = b"hello world\n"      (12)
  seed/b/dup.txt = b"hello world\n"    (12, duplicate content of x.txt)
  seed/a/y.txt = b"unique data here\n" (17)
  => files=3, objects=2, logical_bytes=41, physical_bytes=29
"""

from __future__ import annotations

import pathlib
import tempfile

import pytest

import snapdir


# --------------------------------------------------------------------------- #
# Fixture: the deterministic seed tree shared by every case.
# --------------------------------------------------------------------------- #
def _write_tree(root: pathlib.Path, files: dict[str, bytes]) -> pathlib.Path:
    """Materialize ``{relpath: content}`` under ``root`` and return ``root``."""
    for rel, content in files.items():
        dst = root / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(content)
    return root


# Expected figures for the seed fixture (the cross-language acceptance numbers).
SEED_LOGICAL = 41   # 12 + 12 + 17 (all File entries, dups counted)
SEED_PHYSICAL = 29  # 12 + 17 (unique checksums only)
SEED_FILES = 3
SEED_OBJECTS = 2


@pytest.fixture()
def seed():
    with tempfile.TemporaryDirectory() as d:
        yield _write_tree(
            pathlib.Path(d),
            {
                "a/x.txt": b"hello world\n",       # 12 bytes
                "b/dup.txt": b"hello world\n",     # 12 bytes, duplicate content
                "a/y.txt": b"unique data here\n",  # 17 bytes, unique
            },
        )


def _objects_dir_bytes(store_dir: str | pathlib.Path) -> int:
    """Sum the byte size of every regular file under ``<store_dir>/.objects/``.

    Objects are stored UNCOMPRESSED, so the on-disk total of the content-addressed
    object files must equal ``physical_bytes``. Walk with ``rglob('*')`` and sum
    ``st_size`` for entries that ``is_file()``."""
    objects = pathlib.Path(store_dir) / ".objects"
    return sum(p.stat().st_size for p in objects.rglob("*") if p.is_file())


# --------------------------------------------------------------------------- #
# Case 1 — manifest_size is SYNC and pure over an awaited Manifest.
# --------------------------------------------------------------------------- #
async def test_manifest_size_pins_all_four_fields_as_ints(seed):
    # clause: manifest_size(await manifest(seed)) yields the exact seed figures,
    # every field a plain Python int (u64 -> int, arbitrary precision, NOT float/bool).
    m = await snapdir.manifest(seed)
    stats = snapdir.manifest_size(m)  # SYNC — no await; pure compute over the Manifest.

    # clause: logical_bytes counts ALL File entries incl. the duplicate (12+12+17).
    assert stats.logical_bytes == SEED_LOGICAL
    # clause: physical_bytes counts UNIQUE checksums only (12+17).
    assert stats.physical_bytes == SEED_PHYSICAL
    # clause: files is the count of File entries.
    assert stats.files == SEED_FILES
    # clause: objects is the count of distinct checksums.
    assert stats.objects == SEED_OBJECTS

    # clause: each attribute is a real Python int (never float, never bool).
    for value in (stats.logical_bytes, stats.physical_bytes, stats.files, stats.objects):
        assert isinstance(value, int)
        assert not isinstance(value, bool)


# --------------------------------------------------------------------------- #
# Case 2 — size(store, SnapshotId) of a pushed snapshot equals the manifest figure.
# --------------------------------------------------------------------------- #
async def test_size_of_pushed_snapshot_equals_manifest_size(seed):
    # clause: push seed to a fresh file:// store, then size() of THAT snapshot id
    # must equal manifest_size on all 4 fields (store == manifest for one snapshot).
    with tempfile.TemporaryDirectory() as store_dir:
        store = snapdir.StoreUri("file://" + store_dir)
        pushed_id = await snapdir.push(seed, store)  # returns a 64-hex str.

        want = snapdir.manifest_size(await snapdir.manifest(seed))
        got = await snapdir.size(store, snapdir.SnapshotId(pushed_id))

        assert got.logical_bytes == want.logical_bytes   # clause: logical matches manifest.
        assert got.physical_bytes == want.physical_bytes  # clause: physical matches manifest.
        assert got.files == want.files                    # clause: files matches manifest.
        assert got.objects == want.objects                # clause: objects matches manifest.


# --------------------------------------------------------------------------- #
# Case 3 — size(store, id=None) is the whole deduped store (one snapshot here).
# --------------------------------------------------------------------------- #
async def test_whole_store_size_with_id_none(seed):
    # clause: size(store) with id=None means the whole (deduped) store; with a
    # single pushed snapshot it equals that snapshot's figure. physical==29 absolutely.
    with tempfile.TemporaryDirectory() as store_dir:
        store = snapdir.StoreUri("file://" + store_dir)
        await snapdir.push(seed, store)

        whole = await snapdir.size(store)  # id defaults to None -> whole store.

        assert whole.logical_bytes == SEED_LOGICAL   # clause: whole-store logical == 41.
        assert whole.physical_bytes == SEED_PHYSICAL  # clause: whole-store physical == 29 absolutely.
        assert whole.files == SEED_FILES              # clause: whole-store files == 3.
        assert whole.objects == SEED_OBJECTS          # clause: whole-store objects == 2.


# --------------------------------------------------------------------------- #
# Case 4 — ACCEPTANCE: physical_bytes EQUALS the on-disk .objects/ byte total.
# --------------------------------------------------------------------------- #
async def test_physical_bytes_equals_on_disk_objects_total(seed):
    # clause: the load-bearing acceptance bar — reported physical_bytes must equal
    # the actual summed byte size of every file under <store_dir>/.objects/, and 29.
    with tempfile.TemporaryDirectory() as store_dir:
        store = snapdir.StoreUri("file://" + store_dir)
        await snapdir.push(seed, store)

        reported = (await snapdir.size(store)).physical_bytes
        on_disk = _objects_dir_bytes(store_dir)

        assert reported == on_disk       # clause: report == real on-disk object bytes.
        assert on_disk == SEED_PHYSICAL  # clause: and that total is exactly 29.
        assert reported == SEED_PHYSICAL  # clause: report is exactly 29.


# --------------------------------------------------------------------------- #
# Case 5 — Strict dedup inequalities.
# --------------------------------------------------------------------------- #
async def test_dedup_strict_inequalities(seed):
    # clause: with a genuine duplicate present, dedup MUST bite: logical > physical
    # AND objects < files (a weaker '>=' would let a broken/no-dedup impl pass).
    stats = snapdir.manifest_size(await snapdir.manifest(seed))
    assert stats.logical_bytes > stats.physical_bytes  # clause: dedup saves bytes (41 > 29).
    assert stats.objects < stats.files                 # clause: fewer objects than files (2 < 3).


# --------------------------------------------------------------------------- #
# Case 6 — Store-error contract: (a) missing root raises, (b) empty store zeros,
# (c) unknown scheme raises at StoreUri construction.
# --------------------------------------------------------------------------- #
async def test_size_missing_root_store_raises_store_error():
    # clause (a): a file:// store whose root does NOT exist is an ERROR, not an
    # empty store — size() must raise a SnapdirError (a StoreError) code STORE_ERROR.
    with tempfile.TemporaryDirectory() as d:
        missing = str(pathlib.Path(d) / "does-not-exist-zzz")
        assert not pathlib.Path(missing).exists()
        store = snapdir.StoreUri("file://" + missing)  # construction OK: scheme is valid.
        with pytest.raises(snapdir.SnapdirError) as ei:
            await snapdir.size(store)
        # clause: missing-root maps to the SPECIFIC stable code STORE_ERROR.
        assert ei.value.code == "STORE_ERROR"
        # clause: it is a StoreError (which IS-A SnapdirError) — subtype catch holds.
        assert isinstance(ei.value, snapdir.StoreError)


async def test_size_of_existing_empty_store_is_all_zeros():
    # clause (b): an EXISTING store dir with no manifests pushed returns all-ZERO
    # SizeStats and does NOT raise (empty != missing).
    with tempfile.TemporaryDirectory() as store_dir:
        # store_dir already exists (mkdir by TemporaryDirectory); push nothing.
        store = snapdir.StoreUri("file://" + store_dir)
        stats = await snapdir.size(store)
        assert stats.logical_bytes == 0   # clause: empty store logical == 0.
        assert stats.physical_bytes == 0  # clause: empty store physical == 0.
        assert stats.files == 0           # clause: empty store files == 0.
        assert stats.objects == 0         # clause: empty store objects == 0.


def test_unknown_scheme_store_uri_raises_invalid_store_at_construction():
    # clause (c): an unknown/malformed scheme is rejected AT StoreUri construction
    # with code INVALID_STORE — distinct from the size()-level STORE_ERROR failure.
    with pytest.raises(snapdir.SnapdirError) as ei:
        snapdir.StoreUri("bogus://nope")
    assert ei.value.code == "INVALID_STORE"  # clause: construction-time INVALID_STORE.


# --------------------------------------------------------------------------- #
# Case 7 — Degenerate: a dirs-only Manifest sizes to all zeros.
# --------------------------------------------------------------------------- #
async def test_manifest_size_of_dirs_only_tree_is_all_zeros():
    # clause: a tree with directories but NO File entries has zero files/objects
    # and zero logical/physical bytes (directories contribute no object bytes).
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        (root / "empty_a" / "nested").mkdir(parents=True, exist_ok=True)
        (root / "empty_b").mkdir(parents=True, exist_ok=True)

        m = await snapdir.manifest(root)
        # Guard the fixture premise: the manifest contains no File-typed entries.
        file_entries = [
            e for e in m.entries
            if str(getattr(e.path_type, "name", e.path_type)) == "File"
        ]
        assert file_entries == []  # clause: fixture is genuinely dirs-only.

        stats = snapdir.manifest_size(m)
        assert stats.files == 0           # clause: no File entries -> files == 0.
        assert stats.objects == 0         # clause: no checksums -> objects == 0.
        assert stats.logical_bytes == 0   # clause: no file bytes -> logical == 0.
        assert stats.physical_bytes == 0  # clause: no file bytes -> physical == 0.


# --------------------------------------------------------------------------- #
# Case 8 (adversary hardening, review pass) — cross-snapshot dedup in ONE store.
# --------------------------------------------------------------------------- #
async def test_cross_snapshot_dedup(seed):
    # clause: two DISTINCT snapshots pushed to ONE store must dedup across snapshots.
    # Seed B shares seed A's 12-byte "hello world\n" object and adds one NEW object.
    # All reference figures are COMPUTED from local manifests (no hardcoded counts):
    # whole-store logical/files ADD, but physical_bytes/objects are STRICTLY less than
    # the naive per-snapshot sums because the shared object is stored exactly once.
    with tempfile.TemporaryDirectory() as seed_b_dir, \
            tempfile.TemporaryDirectory() as store_dir:
        seed_b = _write_tree(
            pathlib.Path(seed_b_dir),
            {
                "shared/hello.txt": b"hello world\n",        # shares seed A's 12-byte object
                "fresh/new.txt": b"brand new content!\n",    # distinct object, unique to B
            },
        )

        store = snapdir.StoreUri("file://" + store_dir)
        id_a = snapdir.SnapshotId(await snapdir.push(seed, store))
        id_b = snapdir.SnapshotId(await snapdir.push(seed_b, store))

        # Reference figures computed straight from the two local manifests.
        a = snapdir.manifest_size(await snapdir.manifest(seed))
        b = snapdir.manifest_size(await snapdir.manifest(seed_b))

        assert str(id_a) != str(id_b)  # clause: the two snapshots are genuinely distinct.

        # clause: per-id size() reproduces each snapshot's own manifest figure, exactly.
        size_a = await snapdir.size(store, id_a)
        assert size_a.logical_bytes == a.logical_bytes    # clause: A logical == its manifest.
        assert size_a.physical_bytes == a.physical_bytes  # clause: A physical == its manifest.
        assert size_a.files == a.files                    # clause: A files == its manifest.
        assert size_a.objects == a.objects                # clause: A objects == its manifest.

        size_b = await snapdir.size(store, id_b)
        assert size_b.logical_bytes == b.logical_bytes    # clause: B logical == its manifest.
        assert size_b.physical_bytes == b.physical_bytes  # clause: B physical == its manifest.
        assert size_b.files == b.files                    # clause: B files == its manifest.
        assert size_b.objects == b.objects                # clause: B objects == its manifest.

        # clause: whole-store logical/files are ADDITIVE (every File entry is counted).
        whole = await snapdir.size(store)
        assert whole.files == a.files + b.files                    # clause: files add.
        assert whole.logical_bytes == a.logical_bytes + b.logical_bytes  # clause: logical adds.

        # clause: STRICT cross-snapshot dedup — the shared object is stored ONCE, so the
        # whole-store physical/objects are strictly below the naive per-snapshot sums.
        assert whole.physical_bytes < a.physical_bytes + b.physical_bytes  # clause: bytes deduped.
        assert whole.objects < a.objects + b.objects                       # clause: object deduped.

        # clause: whole-store physical_bytes equals the REAL on-disk .objects/ total.
        assert whole.physical_bytes == _objects_dir_bytes(store_dir)  # clause: report == disk.

        # clause: whole-store size() is deterministic — re-run matches field-by-field.
        again = await snapdir.size(store)
        assert again.logical_bytes == whole.logical_bytes    # clause: logical stable.
        assert again.physical_bytes == whole.physical_bytes  # clause: physical stable.
        assert again.files == whole.files                    # clause: files stable.
        assert again.objects == whole.objects                # clause: objects stable.
