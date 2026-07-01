"""
python_manifest_options.py — BLACK-BOX spec for the NEW manifest/id OPTIONS
surface on the `snapdir` Python binding (Phase 38, gate
`python-manifest-options-spec-tests`, adversary/opus — PM-authored from the
contract; ZERO visibility into bindings/python/src/).

============================================================================
WHAT THIS PINS
----------------------------------------------------------------------------
The binding currently exposes ``manifest(path)`` / ``id(path)`` with NO options.
The fix adds Pythonic keyword-only options mapping to snapdir-api ManifestOptions:

    await manifest(path, *, no_follow=False, absolute=False, exclude=None)
    await id(path,       *, no_follow=False, absolute=False, exclude=None)

Semantics (from the snapdir CLI / snapdir-api ManifestOptions + §1 parity):
  1. no_follow: default FOLLOWS symlinks (dereferences); ``no_follow=True`` records
     the link itself. The two walks of the same tree differ; each is self-consistent
     (id(path, **o) == id_from_manifest(await manifest(path, **o))).
  2. absolute: ``absolute=True`` renders the root/entries as ABSOLUTE paths instead
     of the default ``./``-relative form — distinct raw text + distinct id.
  3. exclude: ``exclude=[RE, ...]`` omits entries matching ANY pattern (OR-combined,
     extended-regex). An excluded entry is ABSENT, a non-excluded one PRESENT.
  4. optionality / backward-compat: omitting options (or all-defaults) == current
     default behavior.
  5. id parity with options: id(path, **o) self-consistent with manifest(path, **o).

EXPECTED-FAIL RATIONALE (no-impl state): the current binding's manifest/id take no
options, so passing these kwargs raises ``TypeError`` (unexpected keyword argument).
That failure IS the no-impl signal. The ``-impl`` gate git mv's this into
bindings/python/tests/ and makes it green; ``-tests-review`` strengthens it.

BLACK-BOX ATTESTATION: authored from the snapdir-api ManifestOptions semantics +
the §1 parity contract + the existing Python test house style. Did NOT read
bindings/python/src/.
============================================================================
"""

import os
import re

import pytest

import snapdir

HEX64 = re.compile(r"\A[0-9a-f]{64}\Z")


# --------------------------------------------------------------------------- #
# A tree with symlinks + exclude fixtures, built in-test.
# --------------------------------------------------------------------------- #
@pytest.fixture()
def tree(tmp_path):
    (tmp_path / "real").mkdir()
    (tmp_path / "real" / "inner.txt").write_bytes(b"inner\n")
    (tmp_path / "target.txt").write_bytes(b"target-bytes\n")
    os.symlink("real", tmp_path / "link_to_dir")        # dir symlink
    os.symlink("target.txt", tmp_path / "link_to_file")  # file symlink
    (tmp_path / "drop.tmp").write_bytes(b"temp\n")
    (tmp_path / "keep.log").write_bytes(b"log\n")
    return tmp_path


def _paths(m):
    """Entry paths of a manifest (attribute access, raw fallback)."""
    try:
        return [e.path for e in m.entries]
    except Exception:  # pragma: no cover - defensive
        return [
            ln.split()[-1]
            for ln in m.raw.splitlines()
            if ln and not ln.startswith("#")
        ]


# --------------------------------------------------------------------------- #
# 1. no_follow vs follow are DISTINCT.
# --------------------------------------------------------------------------- #
async def test_no_follow_yields_a_different_manifest_than_follow(tree):
    followed = await snapdir.manifest(tree)
    nofollow = await snapdir.manifest(tree, no_follow=True)
    assert followed.raw and nofollow.raw
    assert nofollow.raw != followed.raw  # symlink handling must differ


async def test_no_follow_false_equals_default(tree):
    assert (await snapdir.manifest(tree, no_follow=False)).raw == (
        await snapdir.manifest(tree)
    ).raw
    assert await snapdir.id(tree, no_follow=False) == await snapdir.id(tree)


async def test_id_reflects_no_follow_and_differs(tree):
    id_follow = await snapdir.id(tree)
    id_nofollow = await snapdir.id(tree, no_follow=True)
    assert HEX64.match(id_follow) and HEX64.match(id_nofollow)
    assert id_nofollow != id_follow


async def test_each_variant_is_self_consistent(tree):
    for opts in ({}, {"no_follow": True}, {"no_follow": False}):
        m = await snapdir.manifest(tree, **opts)
        direct = await snapdir.id(tree, **opts)
        assert snapdir.id_from_manifest(m) == direct


async def test_no_follow_does_not_recurse_into_a_dir_symlink(tree):
    """Structural: FOLLOW walks INTO link_to_dir (entries under link_to_dir/);
    no_follow records the link as a leaf (no path under link_to_dir/)."""
    followed = _paths(await snapdir.manifest(tree))
    nofollow = _paths(await snapdir.manifest(tree, no_follow=True))
    under = lambda ps: any(re.search(r"(^|/)link_to_dir/", p) for p in ps)
    assert under(followed)
    assert not under(nofollow)
    assert len(followed) > len(nofollow)


# --------------------------------------------------------------------------- #
# 2. absolute option.
# --------------------------------------------------------------------------- #
async def test_absolute_renders_absolute_paths(tree):
    rel = await snapdir.manifest(tree)
    ab = await snapdir.manifest(tree, absolute=True)
    abs_dir = os.path.realpath(tree)
    assert rel.raw != ab.raw
    assert abs_dir in ab.raw
    assert abs_dir not in rel.raw


async def test_absolute_id_differs_and_is_self_consistent(tree):
    id_rel = await snapdir.id(tree)
    id_abs = await snapdir.id(tree, absolute=True)
    assert id_abs != id_rel
    assert snapdir.id_from_manifest(await snapdir.manifest(tree, absolute=True)) == id_abs


# --------------------------------------------------------------------------- #
# 3. exclude option (regex, OR-combined).
# --------------------------------------------------------------------------- #
async def test_exclude_drops_matching_entry_keeps_others(tree):
    full = _paths(await snapdir.manifest(tree))
    assert any(p.endswith("drop.tmp") for p in full)
    filtered = await snapdir.manifest(tree, exclude=[r"\.tmp$"])
    ps = _paths(filtered)
    assert not any(p.endswith("drop.tmp") for p in ps)
    assert any(p.endswith("keep.log") for p in ps)
    assert "drop.tmp" not in filtered.raw
    assert await snapdir.id(tree, exclude=[r"\.tmp$"]) != await snapdir.id(tree)


async def test_exclude_multiple_patterns_or_combine(tree):
    ps = _paths(await snapdir.manifest(tree, exclude=[r"\.tmp$", r"\.log$"]))
    assert not any(p.endswith("drop.tmp") for p in ps)
    assert not any(p.endswith("keep.log") for p in ps)
    assert any(p.endswith("target.txt") for p in ps)


async def test_empty_exclude_equals_default(tree):
    assert (await snapdir.manifest(tree, exclude=[])).raw == (
        await snapdir.manifest(tree)
    ).raw


# --------------------------------------------------------------------------- #
# 4. optionality / backward-compat + combined.
# --------------------------------------------------------------------------- #
async def test_omitting_options_equals_all_defaults(tree):
    a = await snapdir.manifest(tree)
    b = await snapdir.manifest(tree, no_follow=False, absolute=False, exclude=None)
    assert a.raw == b.raw
    assert await snapdir.id(tree) == await snapdir.id(
        tree, no_follow=False, absolute=False, exclude=None
    )


async def test_combined_options_consistent_and_distinct(tree):
    o = {"no_follow": True, "absolute": True, "exclude": [r"\.tmp$"]}
    m = await snapdir.manifest(tree, **o)
    sid = await snapdir.id(tree, **o)
    assert snapdir.id_from_manifest(m) == sid
    assert sid != await snapdir.id(tree)
    assert os.path.realpath(tree) in m.raw
    assert "drop.tmp" not in m.raw


# --------------------------------------------------------------------------- #
# 5. STRENGTHENING (tests-review, adversary/opus via PM) — extended-regex
#    semantics, nested-entry exclude, absolute root-line, option independence.
# --------------------------------------------------------------------------- #
async def test_exclude_drops_a_nested_entry_keeps_its_parent(tree):
    """exclude matches NESTED paths: ``inner`` drops real/inner.txt but the
    parent ``real`` dir (not matched) survives."""
    ps = _paths(await snapdir.manifest(tree, exclude=["inner"]))
    assert not any(p.endswith("inner.txt") for p in ps)
    assert any(p.endswith("real") or "real/" in p for p in ps)


async def test_exclude_is_extended_regex_anchor_and_alternation(tree):
    """ERE semantics (grep -E -v): an end-anchor and alternation both compile."""
    anchored = _paths(await snapdir.manifest(tree, exclude=[r"\.log$"]))
    assert not any(p.endswith("keep.log") for p in anchored)
    assert any(p.endswith("target.txt") for p in anchored)  # not .log → survives
    alt = _paths(await snapdir.manifest(tree, exclude=[r"drop|keep"]))
    assert not any(p.endswith("drop.tmp") for p in alt)
    assert not any(p.endswith("keep.log") for p in alt)


async def test_exclude_is_a_set_filter_same_set_same_id(tree):
    """exclude is a pure membership filter: two patterns selecting the SAME entry
    set yield the SAME id (here both drop only keep.log)."""
    by_anchor = await snapdir.id(tree, exclude=[r"\.log$"])
    by_name = await snapdir.id(tree, exclude=[r"keep\.log$"])
    assert HEX64.match(by_anchor)
    assert by_anchor == by_name


async def test_absolute_root_line_is_the_absolute_dir(tree):
    """The ROOT manifest line specifically renders the absolute tree path under
    ``absolute=True`` (its last column is ``<absdir>/``), vs ``./`` by default."""
    abs_dir = os.path.realpath(tree)

    def root_path_col(m):
        for ln in m.raw.splitlines():
            if ln.startswith("D ") and ln.rstrip().endswith("/"):
                return ln.split()[-1]
        return None

    assert root_path_col(await snapdir.manifest(tree)) == "./"
    assert root_path_col(await snapdir.manifest(tree, absolute=True)) == f"{abs_dir}/"


async def test_absolute_changes_rendering_only_not_membership(tree):
    """absolute reframes paths; it adds/removes no entries (same count + same
    basenames excluding the root)."""
    base = lambda p: p.rstrip("/").split("/")[-1]
    is_root = lambda p: p in ("./", f"{os.path.realpath(tree)}/")
    rel = _paths(await snapdir.manifest(tree))
    ab = _paths(await snapdir.manifest(tree, absolute=True))
    assert len(ab) == len(rel)
    assert sorted(base(p) for p in ab if not is_root(p)) == sorted(
        base(p) for p in rel if not is_root(p)
    )
