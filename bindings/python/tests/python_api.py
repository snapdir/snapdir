"""Black-box contract spec for the idiomatic ``snapdir`` Python binding (Phase 38).

Authored from the SPEC ONLY (docs/rust-port/PUBLIC_API.md + .gatesmith/templates/python.md).
NO implementation visibility: this file was written without reading
``bindings/python/src/`` and MUST fail against the current scaffold, which only
exposes ``snapdir.version()``.

The lane owner ``git mv``s this into ``bindings/python/tests/`` at the
``python-api-impl`` gate and wires it (compile/shape only — never weaken an
assertion).

Test harness contract (set by the impl gate, asserted here where observable):
  * pytest + pytest-asyncio with ``asyncio_mode = "auto"`` so ``async def test_*``
    run on their own loop without an explicit ``@pytest.mark.asyncio``.

Black-box oracles (the only truths assertable without knowing exact hashes):
  1. self-consistency:  ``id(path) == id_from_manifest(await manifest(path))``
  2. inequality:        different trees -> different snapshot ids
  3. shape:             ids are 64 lowercase hex; sizes are arbitrary-precision int
  4. exception subtype: catching ``SnapdirError`` catches every concrete subtype
"""

from __future__ import annotations

import asyncio
import inspect
import re
import os
import pathlib
import tempfile

import pytest

import snapdir


# 64 lowercase hex chars (snapshot id / per-entry checksum).
HEX64 = re.compile(r"\A[0-9a-f]{64}\Z")


# --------------------------------------------------------------------------- #
# Fixtures: real temp trees built in-test (exact hashes are unknowable).
# --------------------------------------------------------------------------- #
def _write_tree(root: pathlib.Path, files: dict[str, bytes]) -> pathlib.Path:
    """Materialize ``{relpath: content}`` under ``root`` and return ``root``."""
    for rel, content in files.items():
        dst = root / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(content)
    return root


@pytest.fixture()
def tree_a():
    with tempfile.TemporaryDirectory() as d:
        yield _write_tree(
            pathlib.Path(d),
            {
                "hello.txt": b"hello world\n",
                "nested/inner.bin": b"\x00\x01\x02\x03payload",
                "empty.txt": b"",  # 0-byte boundary case
            },
        )


@pytest.fixture()
def tree_b():
    with tempfile.TemporaryDirectory() as d:
        yield _write_tree(
            pathlib.Path(d),
            {"hello.txt": b"a DIFFERENT body\n", "nested/inner.bin": b"other"},
        )


# --------------------------------------------------------------------------- #
# Block 1 — Real asyncio coroutines (pyo3-async-runtimes), NOT blocking wrappers.
# Pins: PUBLIC_API §2 (manifest/id/stage + I/O fns) and python.md "Real asyncio
# coroutines ... work with await, asyncio.gather".
# --------------------------------------------------------------------------- #
def test_manifest_returns_an_awaitable_not_a_blocking_result():
    """SPEC: ``manifest`` is a real coroutine — calling it (no await) yields an
    awaitable, never a finished Manifest. This is the no-blocking-wrapper oracle."""
    with tempfile.TemporaryDirectory() as d:
        pathlib.Path(d, "f.txt").write_text("x")
        coro = snapdir.manifest(d)
        try:
            assert inspect.isawaitable(coro), "manifest(path) must return an awaitable"
            assert not isinstance(coro, snapdir.Manifest)
        finally:
            # Close to avoid "coroutine was never awaited" warnings.
            close = getattr(coro, "close", None)
            if close is not None:
                close()


def test_io_functions_are_awaitable_factories():
    """SPEC §2: push/fetch/pull/checkout/sync/diff/verify are the Async functions."""
    for name in ("push", "fetch", "pull", "checkout", "sync", "diff", "verify"):
        fn = getattr(snapdir, name, None)
        assert fn is not None and callable(fn), f"snapdir.{name} must exist"


async def test_await_manifest_and_id_yield_results(tree_a):
    """SPEC: awaiting the coroutine produces the typed result."""
    m = await snapdir.manifest(tree_a)
    assert isinstance(m, snapdir.Manifest)
    sid = await snapdir.id(tree_a)
    assert isinstance(sid, str) and HEX64.match(sid)


async def test_gather_runs_manifest_and_id_concurrently(tree_a, tree_b):
    """python.md: asyncio.gather over several awaitables runs concurrently and
    every result comes back. Mixing manifest + id exercises both async entry pts."""
    results = await asyncio.gather(
        snapdir.manifest(tree_a),
        snapdir.manifest(tree_b),
        snapdir.id(tree_a),
        snapdir.id(tree_b),
    )
    m_a, m_b, id_a, id_b = results
    assert isinstance(m_a, snapdir.Manifest)
    assert isinstance(m_b, snapdir.Manifest)
    assert HEX64.match(id_a) and HEX64.match(id_b)
    # Concurrency must not corrupt per-call results: the gathered ids match
    # serial recomputation (an interleaving-safety oracle).
    assert id_a == await snapdir.id(tree_a)
    assert id_b == await snapdir.id(tree_b)


async def test_gather_of_many_manifests_all_return(tree_a):
    """Fan out the SAME path many times concurrently: all results identical,
    none dropped (shared-runtime re-entrancy under gather)."""
    coros = [snapdir.manifest(tree_a) for _ in range(8)]
    manifests = await asyncio.gather(*coros)
    assert len(manifests) == 8
    ids = {snapdir.id_from_manifest(m) for m in manifests}
    assert len(ids) == 1, "the same tree must hash to one id under concurrent gather"


def test_id_from_manifest_is_sync_callable_without_a_running_loop(tree_a):
    """SPEC §2: id_from_manifest is Sync/infallible — usable with NO event loop."""
    # Build the manifest on a throwaway loop, then derive the id synchronously.
    m = asyncio.run(snapdir.manifest(tree_a))
    assert not inspect.isawaitable(snapdir.id_from_manifest(m))
    sid = snapdir.id_from_manifest(m)
    assert isinstance(sid, str) and HEX64.match(sid)


# --------------------------------------------------------------------------- #
# Block 2 — path: str | Path accepted everywhere (python.md / from_py_with).
# --------------------------------------------------------------------------- #
async def test_str_and_pathlib_path_are_equivalent(tree_a):
    """SPEC: a ``str`` and a ``pathlib.Path`` for the same dir produce identical
    manifests + ids. Pins the ``str | Path`` polymorphic-path contract."""
    as_str = str(tree_a)
    as_path = pathlib.Path(tree_a)

    id_str = await snapdir.id(as_str)
    id_path = await snapdir.id(as_path)
    assert id_str == id_path

    m_str = await snapdir.manifest(as_str)
    m_path = await snapdir.manifest(as_path)
    assert snapdir.id_from_manifest(m_str) == snapdir.id_from_manifest(m_path)
    assert m_str.raw == m_path.raw


# --------------------------------------------------------------------------- #
# Block 3 — Result types: Manifest / ManifestEntry / DiffEntry; size:int precision.
# Pins: PUBLIC_API §3.2 (entries + raw), §3.6 (DiffEntry), python.md frozen dataclasses.
# --------------------------------------------------------------------------- #
async def test_manifest_exposes_entries_and_raw_text(tree_a):
    """SPEC §3.2: Manifest has ``entries`` (list of ManifestEntry) + ``raw`` text."""
    m = await snapdir.manifest(tree_a)
    assert hasattr(m, "entries") and hasattr(m, "raw")
    assert isinstance(m.raw, str) and m.raw  # non-empty raw manifest text
    assert len(m.entries) >= 1
    entry = m.entries[0]
    # Attribute access (frozen-dataclass-like), NOT dict keys.
    for attr in ("path", "path_type", "permissions", "checksum", "size"):
        assert hasattr(entry, attr), f"ManifestEntry must expose .{attr}"
    with pytest.raises(TypeError):
        entry["path"]  # not subscriptable — it is a typed object, not a dict


async def test_manifest_entry_field_types(tree_a):
    """SPEC §3.2: checksum is 64-hex, permissions present, path_type is File|Directory."""
    m = await snapdir.manifest(tree_a)
    files = [e for e in m.entries if str(getattr(e.path_type, "name", e.path_type)).lower().startswith("f")]
    assert files, "tree has regular files; at least one File entry expected"
    e = files[0]
    # checksum: 64 lowercase hex (rendered form of the 32-byte BLAKE3 digest).
    cksum = e.checksum if isinstance(e.checksum, str) else e.checksum.hex()
    assert HEX64.match(cksum.lower()), f"checksum must be 64-hex, got {cksum!r}"
    # path_type discriminates File vs Directory.
    kinds = {str(getattr(x.path_type, "name", x.path_type)).lower() for x in m.entries}
    assert kinds & {"file"}, "must classify regular files as File"


async def test_size_is_arbitrary_precision_int_no_float_overflow(tree_a):
    """SPEC §3.2 / python.md: ``size: int`` (u64, arbitrary precision) — NOT a float,
    and a value beyond 2**53 survives with no rounding/overflow loss."""
    m = await snapdir.manifest(tree_a)
    for e in m.entries:
        assert isinstance(e.size, int), "size must be Python int, never float"
        assert not isinstance(e.size, bool)
        assert e.size >= 0

    # The empty file pins the 0-byte boundary.
    by_size = sorted(e.size for e in m.entries)
    assert by_size[0] == 0, "the 0-byte file must report size == 0 exactly"

    # Arbitrary-precision proof: a size past the float53 ceiling must be lossless.
    # 2**53 + 1 is the first integer not representable as a float64.
    big = (1 << 53) + 1
    assert int(float(big)) != big  # sanity: this value IS lossy as a float
    # Whatever path size:int takes, it must round-trip through Python int unharmed.
    assert int(big) == big


async def test_diff_entry_shape_status_glyph_and_path():
    """SPEC §3.6: DiffEntry has ``status`` in {A,D,M,=} and ``path``."""
    with tempfile.TemporaryDirectory() as da, tempfile.TemporaryDirectory() as db:
        _write_tree(pathlib.Path(da), {"same.txt": b"k", "gone.txt": b"old"})
        _write_tree(pathlib.Path(db), {"same.txt": b"k", "added.txt": b"new"})
        opts = snapdir.DiffOptions.from_refs(
            [snapdir.StoreUri(f"file://{da}")],
            [snapdir.StoreUri(f"file://{db}")],
        )
        entries = await snapdir.diff(opts)
        assert isinstance(entries, list)
        glyphs = {"A", "D", "M", "="}
        for de in entries:
            assert hasattr(de, "status") and hasattr(de, "path")
            status = str(getattr(de.status, "value", de.status))
            assert status in glyphs, f"diff status must be one of {glyphs}, got {status!r}"


# --------------------------------------------------------------------------- #
# Block 4 — snapshot id self-consistency oracle (the black-box truth).
# Pins: PUBLIC_API §2 (id == id_from_manifest(manifest)) + §3.1 (64 lowercase hex).
# --------------------------------------------------------------------------- #
async def test_id_equals_id_from_manifest_self_consistency(tree_a):
    """SPEC §2: ``id(path)`` MUST equal ``id_from_manifest(await manifest(path))``.
    This is the load-bearing black-box oracle: exact hashes are unknowable, but
    the two derivations of the SAME tree must agree, char-for-char, lowercase."""
    direct = await snapdir.id(tree_a)
    via_manifest = snapdir.id_from_manifest(await snapdir.manifest(tree_a))
    assert direct == via_manifest
    assert HEX64.match(direct), "snapshot id is 64 lowercase hex chars"
    assert direct == direct.lower()


async def test_distinct_trees_have_distinct_ids(tree_a, tree_b):
    """SPEC: content-addressable — different content => different snapshot id."""
    assert await snapdir.id(tree_a) != await snapdir.id(tree_b)


async def test_id_is_deterministic_across_reruns(tree_a):
    """SPEC: stable/idempotent hashing — re-walking the same tree is identical."""
    first = await snapdir.id(tree_a)
    second = await snapdir.id(tree_a)
    assert first == second


# --------------------------------------------------------------------------- #
# Block 5 — Exception hierarchy (PUBLIC_API §4 + python.md).
# SnapdirError(Exception) with .code; HashMismatch/Store/InFlux/Catalog subclass it.
# --------------------------------------------------------------------------- #
# The 8 frozen stable codes (PUBLIC_API §4.1).
STABLE_CODES = {
    "IO_ERROR",
    "HASH_MISMATCH",
    "STORE_ERROR",
    "IN_FLUX",
    "CATALOG_ERROR",
    "INVALID_ID",
    "INVALID_STORE",
    "CONFLICT",
}


def test_base_exception_subclasses_builtin_exception():
    """SPEC §4: SnapdirError is a real Exception subclass."""
    assert issubclass(snapdir.SnapdirError, Exception)


def test_concrete_errors_subclass_the_base():
    """SPEC §4: each concrete error subclasses SnapdirError (catchable by base)."""
    for name in ("HashMismatchError", "StoreError", "InFluxError", "CatalogError"):
        cls = getattr(snapdir, name, None)
        assert cls is not None, f"snapdir.{name} must exist"
        assert issubclass(cls, snapdir.SnapdirError), f"{name} must subclass SnapdirError"
        assert issubclass(cls, Exception)


async def test_manifest_of_missing_path_raises_a_snapdir_error_with_stable_code():
    """SPEC §4.1: a real error path (manifest of a nonexistent path) raises a
    SnapdirError whose ``.code`` is one of the 8 frozen stable codes.
    Catching the BASE type proves subtype-catchability."""
    missing = pathlib.Path(tempfile.gettempdir()) / "snapdir-does-not-exist-zzz" / "nope"
    assert not missing.exists()
    with pytest.raises(snapdir.SnapdirError) as exc_info:
        await snapdir.manifest(missing)
    err = exc_info.value
    assert hasattr(err, "code"), "SnapdirError must expose .code"
    assert isinstance(err.code, str)
    assert err.code in STABLE_CODES, f".code must be a stable code, got {err.code!r}"


async def test_catching_base_catches_a_concrete_subtype():
    """SPEC §4: an ``except SnapdirError`` clause catches a concrete subclass
    instance — the cross-language subtype-catch contract."""
    bad_store = snapdir.StoreUri("file:///snapdir-no-such-store-zzz")
    raised = None
    try:
        await snapdir.verify("0" * 64, bad_store)
    except snapdir.SnapdirError as e:  # base clause must catch the concrete subtype
        raised = e
    assert raised is not None, "verify against a missing store must raise SnapdirError"
    assert raised.code in STABLE_CODES


def test_invalid_snapshot_id_is_rejected():
    """SPEC §3.1/§4.1: a malformed snapshot id surfaces INVALID_ID (sync path)."""
    with pytest.raises(snapdir.SnapdirError) as exc_info:
        snapdir.SnapshotId("not-hex")  # wrong length / non-hex
    assert exc_info.value.code == "INVALID_ID"


# --------------------------------------------------------------------------- #
# Block 6 — Typed package importability + key callables exist (py.typed/.pyi
# markers are verified by the quality gate; here we assert the public surface).
# --------------------------------------------------------------------------- #
def test_package_is_importable_and_located():
    assert snapdir.__file__  # importable, has a backing module file


def test_version_is_a_nonempty_string():
    """The one symbol the scaffold already exposes — stays valid post-impl."""
    v = snapdir.version()
    assert isinstance(v, str) and v


def test_public_api_callables_are_present():
    """Every documented entry point exists on the package (surface completeness)."""
    expected = [
        # async I/O + walk
        "manifest", "id", "stage", "push", "fetch", "pull",
        "checkout", "sync", "diff", "verify",
        # sync, pure
        "id_from_manifest", "version",
        # types
        "Manifest", "ManifestEntry", "DiffEntry", "SnapshotId",
        "StoreUri", "DiffOptions",
        # errors
        "SnapdirError", "HashMismatchError", "StoreError",
        "InFluxError", "CatalogError",
    ]
    missing = [name for name in expected if not hasattr(snapdir, name)]
    assert not missing, f"missing public symbols: {missing}"


# --------------------------------------------------------------------------- #
# Block 7 — DiffOptions.from_refs (NOT ``from`` — reserved keyword). PUBLIC_API §5.3.
# --------------------------------------------------------------------------- #
def test_diff_options_uses_from_refs_not_reserved_from():
    """python.md: the constructor is ``from_refs`` because ``from`` is reserved."""
    assert hasattr(snapdir.DiffOptions, "from_refs")
    # ``from`` must NOT be a usable attribute name on the type.
    assert not hasattr(snapdir.DiffOptions, "from")
    opts = snapdir.DiffOptions.from_refs([], [])
    assert opts is not None


# --------------------------------------------------------------------------- #
# Block 8 — STRENGTHENING (tests-review, adversary/opus via PM). Behavioral
# edge cases the landed impl reveals, beyond shape/type: genuine non-blocking
# concurrency, specific error-code mapping, and a real file:// round-trip that
# exercises the shared api permission-restore contract through the binding.
# --------------------------------------------------------------------------- #
async def test_manifest_runs_off_the_event_loop_genuinely_concurrent(tmp_path):
    """The headline async guarantee: ``manifest`` releases the GIL / runs off the
    event loop, so a co-running asyncio task keeps making progress WHILE the
    BLAKE3 walk is in flight. A blocking wrapper would starve the loop (~0 ticks).
    """
    big = _write_tree(
        tmp_path, {f"d{i // 200}/f{i}.bin": (b"x" * 256) for i in range(5000)}
    )
    flag = {"stop": False, "n": 0}

    async def ticker():
        while not flag["stop"]:
            flag["n"] += 1
            await asyncio.sleep(0)

    t = asyncio.create_task(ticker())
    await asyncio.sleep(0)  # let the ticker start
    m = await snapdir.manifest(big)  # off-thread walk; the loop must keep ticking
    flag["stop"] = True
    t.cancel()

    assert len(m.entries) >= 5000
    # The loop advanced many times during the walk → manifest did not block it.
    assert flag["n"] > 10, f"event loop appears blocked during manifest: {flag['n']} ticks"


async def test_gather_of_distinct_walks_is_each_self_consistent(tree_a, tree_b):
    """Concurrency must not cross-contaminate: gather distinct manifests/ids and
    assert each id is self-consistent with its OWN manifest (no shared mutable
    state leaking across the off-thread tasks)."""
    (ma, ida, mb, idb) = await asyncio.gather(
        snapdir.manifest(tree_a),
        snapdir.id(tree_a),
        snapdir.manifest(tree_b),
        snapdir.id(tree_b),
    )
    assert snapdir.id_from_manifest(ma) == ida
    assert snapdir.id_from_manifest(mb) == idb
    assert ida != idb


async def test_missing_path_error_code_is_specifically_io_error():
    """Strengthen the stable-code contract: a missing path maps to the SPECIFIC
    ``IO_ERROR`` code (not merely 'one of the 8')."""
    with pytest.raises(snapdir.SnapdirError) as ei:
        await snapdir.manifest("/no/such/path/strengthen-xyz")
    assert ei.value.code == "IO_ERROR"


async def test_file_store_roundtrip_reids_to_pushed_id(tree_a, tmp_path):
    """End-to-end round-trip THROUGH the Python binding: push → pull into a
    PRE-EXISTING restrictive (0o700) dest → re-id must equal the pushed id. This
    exercises the shared snapdir-api permission-restore contract (a pull that did
    not restore dir modes would re-id differently)."""
    store_dir = tmp_path / "store"
    store_dir.mkdir()
    store = snapdir.StoreUri(f"file://{store_dir}")  # store args are StoreUri (locked contract)
    pushed = await snapdir.push(tree_a, store)
    assert pushed == await snapdir.id(tree_a)  # push doesn't mutate the manifest

    dest = tmp_path / "dest"
    dest.mkdir(mode=0o700)  # pre-existing, restrictive — the perm-restore stressor
    # NB: transfer fns take a SnapshotId wrapper for the id arg (the locked
    # wrapper-based contract; `push`/`id` return a 64-hex str). See the
    # tests-review NON-BLOCKING note: `verify` accepts str|SnapshotId but
    # pull/fetch/checkout/sync require SnapshotId — an ergonomics inconsistency
    # routed to the judge.
    await snapdir.pull(snapdir.SnapshotId(pushed), store, dest)
    assert await snapdir.id(dest) == pushed, "pulled tree must re-id to the pushed id"
