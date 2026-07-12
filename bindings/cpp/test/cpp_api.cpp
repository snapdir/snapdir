// cpp_api.cpp — black-box C++ spec for the snapdir C++ RAII binding
// (Phase 40, gate cpp-api-spec-tests; adversary/opus).
//
// Authored from the SPEC ONLY: the FROZEN C ABI (include/snapdir.h,
// c-abi.sha.lock) + the PUBLIC declarations of bindings/cpp/include/snapdir.hpp.
// NO visibility into the binding's implementation or the Rust src/. It is a
// self-contained C++20 test with its own tiny assert harness (the image is not
// guaranteed to ship a test framework). It is EXTERNAL (it calls only the
// public snapdir:: surface) and is EXPECTED TO FAIL / be incomplete against the
// current scaffold (the diff-JSON parser is minimal and id-options routing is
// unproven) — cpp-api-impl makes it pass. Do NOT weaken assertions to pass
// against the scaffold.
//
// Build (inside the snapdir-bindings image): `make -C bindings/cpp test`, which
// runs (after `cargo build --release -p snapdir-ffi` vendors the staticlib):
//   clang++ -std=c++20 -Wall -Wextra -Werror -Iinclude test/cpp_api.cpp
//     -Llib -lsnapdir_ffi -lpthread -ldl -lm -o build/cpp_api_test
// (trailing-backslash line continuations avoided so g++ -Wcomment stays clean)
// Run hermetic + offline (file:// only):
//   SNAPDIR_CACHE_DIR=$T/cache SNAPDIR_CATALOG_DB_PATH=$T/catalog.db /tmp/cpp_api
// Memory contract (the paramount axis):
//   valgrind --leak-check=full --error-exitcode=99 /tmp/cpp_api   # must be clean
//
// The contract this pins (from snapdir.hpp PUBLIC surface + snapdir.h):
//   std::string              snapdir::version();
//   Manifest                 snapdir::manifest(path, ManifestOptions);
//   std::string              snapdir::id(path, ManifestOptions);
//   std::string              snapdir::id_from_manifest(Manifest);
//   std::future<std::string> snapdir::push(path, store_uri, PushOptions);
//   std::future<void>        snapdir::pull(id, store_uri, dest, PullOptions);
//   std::future<void>        snapdir::fetch(id, store_uri, FetchOptions);
//   std::future<std::vector<DiffEntry>> snapdir::diff(from_uri, to_uri, DiffOptions);
//   snapdir::Error : std::runtime_error, with .code() ∈ the 8 stable codes (+ INTERNAL)
//   ManifestEntry{type:PathType, perm:uint32, checksum:string, size:uint64, path}
//   DiffEntry{status:DiffStatus, path}

#include <snapdir.hpp>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <future>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include <sys/stat.h>
#include <unistd.h>

namespace fs = std::filesystem;

// ─── tiny test harness ─────────────────────────────────────────────────────
// Records failures and keeps going so one run surfaces every broken axis.

static int g_failures = 0;
static int g_checks   = 0;

#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        ++g_checks;                                                             \
        if (!(cond)) {                                                          \
            ++g_failures;                                                       \
            std::fprintf(stderr, "FAIL %s:%d: %s  (%s)\n", __FILE__, __LINE__,  \
                         (msg), #cond);                                         \
        }                                                                       \
    } while (0)

// ─── the 8 stable ABI error codes (snapdir.h / snapdir_error_code) ──────────
// A binding failure MUST surface .code() from exactly this set (or "INTERNAL"
// for a catch_unwind boundary). NOT a message, NOT empty.
static const std::set<std::string> kStableCodes = {
    "IO_ERROR", "HASH_MISMATCH", "STORE_ERROR", "IN_FLUX",
    "CATALOG_ERROR", "INVALID_ID", "INVALID_STORE", "CONFLICT",
};

static bool is_stable_code(const std::string &c) {
    return kStableCodes.count(c) != 0 || c == "INTERNAL";
}

static bool is_hex64(const std::string &s) {
    if (s.size() != 64) return false;
    for (char ch : s) {
        const bool ok = (ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f');
        if (!ok) return false; // 64 LOWERCASE hex
    }
    return true;
}

// Unique per-process temp root so concurrent CI lanes don't collide. We avoid
// $TMPDIR pollution by rooting everything under one mkdtemp dir we clean up.
static fs::path make_tmp_root() {
    std::string tmpl = (fs::temp_directory_path() /
                        ("snapdir-cpp-" + std::to_string(::getpid()) + "-XXXXXX"))
                           .string();
    std::vector<char> buf(tmpl.begin(), tmpl.end());
    buf.push_back('\0');
    char *p = ::mkdtemp(buf.data());
    if (!p) {
        std::perror("mkdtemp");
        std::exit(2);
    }
    return fs::path(p);
}

// Build a small offline tree: a.txt (0644), sub/ dir (0755), sub/b.bin (0600),
// link -> a.txt (symlink). Returns the tree root.
static fs::path build_tree(const fs::path &root) {
    fs::create_directories(root / "sub");
    {
        std::ofstream(root / "a.txt") << "hello";
    }
    {
        std::ofstream(root / "sub" / "b.bin") << "world!\n";
    }
    ::chmod((root / "a.txt").c_str(), 0644);
    ::chmod((root / "sub").c_str(), 0755);
    ::chmod((root / "sub" / "b.bin").c_str(), 0600);
    // Symlink for the no_follow axis; ignore if the FS rejects it.
    std::error_code ec;
    fs::create_symlink("a.txt", root / "link", ec);
    return root;
}

// ─── 1. RAII frees on the THROW path (the PARAMOUNT axis) ───────────────────
//
// snapdir.hpp's contract (its own header banner): "Every C allocation is freed
// even when an Error is thrown: StringGuard and ErrorGuard are the only two RAII
// owners of C heap memory." manifest()/id() on a missing path go through
// snapdir_manifest/snapdir_id which set *err_out (a heap SnapdirError) and
// return NULL; throw_if_set() must construct the Error, FREE the SnapdirError,
// and throw — leaking neither the error nor any partial string. valgrind
// --leak-check=full --error-exitcode runs this; a per-throw leak here trips it.
//
// We exercise the throw path 200× in a loop so any per-throw leak is unmistakable
// to memcheck (definitely + indirectly + possibly lost must all be 0).
static void test_raii_frees_on_throw() {
    const fs::path missing = "/snapdir/no/such/path/zzz-does-not-exist-xyz";

    int thrown = 0;
    for (int i = 0; i < 200; ++i) {
        bool caught = false;
        try {
            // id() on a missing dir MUST throw (the C fn sets *err_out + NULLs).
            std::string s = snapdir::id(missing);
            (void)s; // unreachable on the contract; if reached the binding is wrong
        } catch (const snapdir::Error &e) {
            caught = true;
            ++thrown;
            // The message must be non-empty and the code must be stable — proves
            // the Error copied BOTH borrowed C strings before the guard freed them
            // (a use-after-free of the freed SnapdirError would corrupt these).
            CHECK(std::strlen(e.what()) > 0, "Error::what() must be non-empty on throw");
            CHECK(is_stable_code(e.code()), "Error::code() must be a stable ABI code");
        } catch (...) {
            CHECK(false, "id(missing) threw a non-snapdir::Error type");
        }
        CHECK(caught, "id() on a missing path MUST throw snapdir::Error");
    }
    CHECK(thrown == 200, "every iteration of the throw loop must have thrown");

    // Same for manifest() — a different C entry point with the same guard wiring.
    bool caught = false;
    try {
        snapdir::Manifest m = snapdir::manifest(missing);
        (void)m;
    } catch (const snapdir::Error &e) {
        caught = true;
        CHECK(is_stable_code(e.code()), "manifest(missing).code() must be stable");
    } catch (...) {
        CHECK(false, "manifest(missing) threw a non-snapdir::Error type");
    }
    CHECK(caught, "manifest() on a missing path MUST throw snapdir::Error");
}

// ─── 2. Error.code() maps to the C stable code (not a message) ──────────────
//
// The missing-path failure's .code() must be a NON-EMPTY member of the 8-code
// set — the canonical missing-path code is IO_ERROR. We assert membership
// strictly and note (do not hard-require) IO_ERROR so the test pins the CONTRACT
// (stable enum) without over-fitting the exact mapping.
static void test_error_code_is_stable() {
    try {
        snapdir::id("/snapdir/definitely/not/here/abc");
        CHECK(false, "id() on a missing path did not throw");
    } catch (const snapdir::Error &e) {
        CHECK(!e.code().empty(), "Error::code() must not be empty");
        CHECK(is_stable_code(e.code()), "Error::code() must be one of the 8 stable codes / INTERNAL");
        // .code() must be a CODE, never the human message. The message lives in
        // what(); the code must not equal the full formatted message.
        CHECK(e.code() != std::string(e.what()),
              "Error::code() must be the stable code, not the message");
        if (e.code() != "IO_ERROR") {
            std::fprintf(stderr, "note: missing-path code = %s (expected IO_ERROR)\n",
                         e.code().c_str());
        }
    } catch (...) {
        CHECK(false, "non-snapdir::Error thrown for a missing path");
    }
}

// ─── 3. manifest()/id() return values + self-consistency ────────────────────
//
// id(tree) is 64 lowercase hex; id_from_manifest(manifest(tree)) == id(tree);
// manifest(tree).entries is non-empty; a known entry carries the expected
// uint64 size, a 'D'/'F'/'L' type, and an octal perm. Determinism: two id()
// calls over the unchanged tree are identical.
static void test_manifest_and_id_self_consistency(const fs::path &tree) {
    const std::string id1 = snapdir::id(tree);
    CHECK(is_hex64(id1), "id(tree) must be 64 lowercase hex");

    const std::string id2 = snapdir::id(tree);
    CHECK(id1 == id2, "id() must be deterministic over an unchanged tree");

    snapdir::Manifest m = snapdir::manifest(tree);
    CHECK(!m.raw.empty(), "Manifest.raw must hold the raw manifest text");
    CHECK(!m.entries.empty(), "Manifest.entries must be non-empty for a non-empty tree");

    const std::string id_from_m = snapdir::id_from_manifest(m);
    CHECK(is_hex64(id_from_m), "id_from_manifest must be 64 lowercase hex");
    CHECK(id_from_m == id1,
          "id_from_manifest(manifest(tree)) must equal id(tree)");

    // Inspect the known entries. a.txt is a 5-byte 0644 file; sub is a 0755 dir;
    // sub/b.bin is a 7-byte 0600 file. We match on a normalized path string
    // (strip a leading "./" and any trailing "/") so the manifest's path
    // conventions — "./"-relative roots and trailing-slash directory entries —
    // don't matter to the matcher. (filename() is empty for "./sub/".)
    auto norm = [](const fs::path &p) {
        std::string s = p.generic_string();
        if (s.rfind("./", 0) == 0) s.erase(0, 2);
        while (!s.empty() && s.back() == '/') s.pop_back();
        return s;
    };
    bool saw_a = false, saw_sub = false, saw_b = false;
    for (const auto &e : m.entries) {
        const std::string rel = norm(e.path);
        // perm must be a sane octal mode (low 12 bits at most).
        CHECK((e.perm & ~0x0fffu) == 0, "entry perm must be octal mode bits");
        if (e.type == snapdir::PathType::File) {
            CHECK(is_hex64(e.checksum), "file entry checksum must be 64-hex");
        }
        if (rel == "a.txt") {
            saw_a = true;
            CHECK(e.type == snapdir::PathType::File, "a.txt must be type F");
            CHECK(e.size == 5u, "a.txt size must be 5 (uint64)");
            CHECK((e.perm & 0777u) == 0644u, "a.txt perm must be 0644");
        } else if (rel == "sub") {
            saw_sub = true;
            CHECK(e.type == snapdir::PathType::Directory, "sub must be type D");
        } else if (rel == "sub/b.bin") {
            saw_b = true;
            CHECK(e.type == snapdir::PathType::File, "b.bin must be type F");
            CHECK(e.size == 7u, "b.bin size must be 7 (uint64)");
            CHECK((e.perm & 0777u) == 0600u, "b.bin perm must be 0600");
        }
        // uint64 width: adding 2^40 must not overflow the field's type.
        const std::uint64_t huge = e.size + (std::uint64_t(1) << 40);
        CHECK(huge >= e.size, "ManifestEntry.size must really be uint64");
    }
    CHECK(saw_a, "manifest must list a.txt");
    CHECK(saw_sub, "manifest must list sub/ as a directory");
    CHECK(saw_b, "manifest must list sub/b.bin");
}

// ─── 4. options honoured (exclude → id(); no_follow → manifest()) ────────────
//
// exclude IS a parameter of the C ABI snapdir_id (snapdir.h), so it must change
// the id() of the default walk. no_follow is NOT a snapdir_id parameter — it
// lives only on snapdir_manifest — so the contract-correct black-box assertion
// is that no_follow changes the MANIFEST (and therefore id_from_manifest), and
// that under no_follow the symlink target is not dereferenced into a regular
// file. We do NOT assert id() honours no_follow (the C ABI cannot route it).
static void test_options_honoured(const fs::path &tree) {
    const std::string base = snapdir::id(tree);
    CHECK(is_hex64(base), "base id must be 64-hex");

    snapdir::ManifestOptions exo;
    exo.exclude = "a\\.txt"; // extended-regex; drops a.txt from the id() walk
    const std::string excluded = snapdir::id(tree, exo);
    CHECK(is_hex64(excluded), "excluded id must be 64-hex");
    CHECK(excluded != base, "exclude option must change the snapshot id() (snapdir_id param)");

    // no_follow: only meaningful if the symlink got created.
    if (fs::is_symlink(tree / "link")) {
        snapdir::ManifestOptions def, nfo;
        nfo.no_follow = true;

        snapdir::Manifest mdef = snapdir::manifest(tree, def);
        snapdir::Manifest mnf  = snapdir::manifest(tree, nfo);

        CHECK(mdef.raw != mnf.raw,
              "no_follow must change the manifest text (symlink handled differently)");

        const std::string idmdef = snapdir::id_from_manifest(mdef);
        const std::string idmnf  = snapdir::id_from_manifest(mnf);
        CHECK(is_hex64(idmdef) && is_hex64(idmnf), "manifest-ids must be 64-hex");
        CHECK(idmdef != idmnf,
              "no_follow must change id_from_manifest vs the default follow walk");

        // RESTORED (tests-review): the staged spec asserted that id() itself
        // honours no_follow. cpp-api-impl removed this on the (true-for-the-C-ABI
        // but false-for-THIS-BINDING) premise that snapdir_id cannot route
        // no_follow. The BINDING's id() (snapdir.hpp) explicitly routes
        // no_follow/absolute through manifest()→id_from_manifest(), so this is a
        // real public-API contract: id(tree, no_follow) MUST differ from the
        // default follow walk and MUST equal id_from_manifest(manifest(no_follow)).
        // Restoring it pins that the binding's id() compensation actually fires.
        const std::string idnf = snapdir::id(tree, nfo);
        CHECK(is_hex64(idnf), "no_follow id() must be 64-hex");
        CHECK(idnf != base,
              "id(tree, no_follow) must change vs the default follow walk (binding routes via manifest)");
        CHECK(idnf == idmnf,
              "id(tree, no_follow) must equal id_from_manifest(manifest(tree, no_follow))");

        // WEAKENING-AUDIT FINDING (tests-review): I initially restored the staged
        // spec's stronger "must record 'link' as a Symlink (L)" assertion, ran it,
        // and it FAILED — but NOT because of an impl bug. The FROZEN oracle's golden
        // tests/golden/expected/symlinks-nofollow.manifest proves snapdir's manifest
        // model has NO type-L representation: under --no-follow a relative/dangling
        // symlink is OMITTED entirely (not recorded as L, not as a dereferenced F).
        // So the spec's 'L' assumption was a WRONG black-box guess about snapdir's
        // symlink model, and cpp-api-impl's re-word to "not a dereferenced File"
        // (which 'link' satisfies by being ABSENT) is a legitimate RE-ADDRESS of a
        // wrong assumption, NOT a weakening. The contract-correct, oracle-backed
        // assertion is therefore the landed one, which we keep:
        bool link_as_file = false;
        for (const auto &e : mnf.entries) {
            if (e.path.filename().string() == "link" &&
                e.type == snapdir::PathType::File) {
                link_as_file = true;
            }
        }
        CHECK(!link_as_file,
              "under no_follow the symlink must not be recorded as a dereferenced File");
    } else {
        std::fprintf(stderr, "note: symlink unsupported here; skipping no_follow check\n");
    }
}

// ─── 5. async std::future round-trip (push → pull → fetch; failing .get()) ──
//
// push(tree, file:// store).get() returns an id == id(tree); pull(id, store,
// dest).get() into a PRE-EXISTING 0700 dest succeeds and re-id(dest) == pushed
// (exercises the shared permission-restore contract THROUGH the C ABI, exactly
// like the Go/Python round-trips). fetch(id, store).get() succeeds. A future
// wrapping a failing op (pull of a bogus id) re-throws snapdir::Error from .get().
static void test_async_roundtrip(const fs::path &tree, const fs::path &root) {
    const std::string store_uri =
        "file://" + (root / ("store-" + std::to_string(::getpid()))).string();

    const std::string local = snapdir::id(tree);

    // push() returns std::future<std::string>.
    std::future<std::string> push_fut = snapdir::push(tree, store_uri);
    std::string pushed;
    try {
        pushed = push_fut.get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push().get() threw: ") + e.what()).c_str());
        return;
    }
    CHECK(is_hex64(pushed), "push() id must be 64-hex");
    CHECK(pushed == local, "push() id must equal local id(tree) (push must not mutate)");

    // pull() into a PRE-EXISTING 0700 dest. A pull that didn't restore each
    // entry's mode would re-id differently.
    const fs::path dest = root / "dest";
    fs::create_directories(dest);
    ::chmod(dest.c_str(), 0700);

    std::future<void> pull_fut = snapdir::pull(pushed, store_uri, dest);
    try {
        pull_fut.get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("pull().get() threw: ") + e.what()).c_str());
        return;
    }
    const std::string reid = snapdir::id(dest);
    CHECK(reid == pushed,
          "pulled tree must re-id to the pushed id (permission-restore via C ABI)");

    // fetch() into the local cache must succeed for a present snapshot.
    std::future<void> fetch_fut = snapdir::fetch(pushed, store_uri);
    try {
        fetch_fut.get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("fetch().get() threw: ") + e.what()).c_str());
    }

    // A future wrapping a FAILING op must re-throw snapdir::Error from .get()
    // (the async lambda's ErrorGuard fires across the thread boundary). Pulling a
    // bogus id from an empty store is the canonical failure.
    const std::string empty_store =
        "file://" + (root / "empty-store").string();
    const std::string bogus_id(64, '0');
    const fs::path bad_dest = root / "bad-dest";
    fs::create_directories(bad_dest);

    std::future<void> bad_fut = snapdir::pull(bogus_id, empty_store, bad_dest);
    bool threw = false;
    try {
        bad_fut.get();
    } catch (const snapdir::Error &e) {
        threw = true;
        CHECK(is_stable_code(e.code()),
              "pull(bogus).get() error code must be a stable ABI code");
    } catch (...) {
        CHECK(false, "pull(bogus).get() threw a non-snapdir::Error type");
    }
    CHECK(threw, "a future wrapping a failing pull must re-throw snapdir::Error from .get()");
}

// ─── 6. diff() across two stores ─────────────────────────────────────────────
//
// push tree A → storeA, a MODIFIED tree B → storeB; diff(storeA, storeB).get()
// returns DiffEntry(s) with the expected glyph(s) and the changed path. A
// self-diff (same store both sides) is empty (no A/D/M rows). file:// only.
static void test_diff_across_stores(const fs::path &tree, const fs::path &root) {
    const std::string storeA =
        "file://" + (root / "diff-storeA").string();
    const std::string storeB =
        "file://" + (root / "diff-storeB").string();

    // A = the base tree.
    try {
        snapdir::push(tree, storeA).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push A threw: ") + e.what()).c_str());
        return;
    }

    // B = a modified copy: change a.txt's bytes and add c.txt.
    const fs::path treeB = root / "treeB";
    fs::create_directories(treeB / "sub");
    { std::ofstream(treeB / "a.txt") << "HELLO-CHANGED"; }   // modified content
    { std::ofstream(treeB / "sub" / "b.bin") << "world!\n"; } // unchanged
    { std::ofstream(treeB / "c.txt") << "added\n"; }          // added
    ::chmod((treeB / "a.txt").c_str(), 0644);
    ::chmod((treeB / "sub" / "b.bin").c_str(), 0600);
    ::chmod((treeB / "c.txt").c_str(), 0644);
    try {
        snapdir::push(treeB, storeB).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push B threw: ") + e.what()).c_str());
        return;
    }

    // A→B diff: must report at least the added c.txt and the modified a.txt.
    std::vector<snapdir::DiffEntry> entries;
    try {
        entries = snapdir::diff(storeA, storeB).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("diff(A,B).get() threw: ") + e.what()).c_str());
        return;
    }
    CHECK(!entries.empty(), "diff across two distinct stores must report changes");

    bool saw_added_c = false, saw_modified_a = false;
    for (const auto &e : entries) {
        const char g = static_cast<char>(e.status);
        CHECK(g == 'A' || g == 'D' || g == 'M' || g == '=',
              "DiffEntry.status must be one of A/D/M/=");
        const std::string name = e.path.filename().string();
        if (e.status == snapdir::DiffStatus::Added && name == "c.txt") saw_added_c = true;
        if (e.status == snapdir::DiffStatus::Modified && name == "a.txt") saw_modified_a = true;
    }
    CHECK(saw_added_c, "diff must report c.txt as Added (A)");
    CHECK(saw_modified_a, "diff must report a.txt as Modified (M)");

    // self-diff: same store both sides → no change rows.
    std::vector<snapdir::DiffEntry> self;
    try {
        self = snapdir::diff(storeA, storeA).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("self diff threw: ") + e.what()).c_str());
        return;
    }
    for (const auto &e : self) {
        CHECK(e.status == snapdir::DiffStatus::Unchanged,
              "a self-diff must produce no Added/Deleted/Modified rows");
    }
}

// ─── 7. async throw-path RAII no-leak in a loop (strengthening) ──────────────
//
// test_raii_frees_on_throw exercises the SYNC throw path 200×. The async path
// has its OWN guard wiring: each std::async lambda constructs its ErrorGuard,
// the C op fails, throw_if_set() frees the SnapdirError and throws ACROSS the
// std::async thread boundary, and std::future stores+re-throws it at .get().
// A per-throw leak in the async lambda (e.g. an ErrorGuard that did not fire on
// the throw path) would be just as real as a sync leak but is NOT covered above.
// We pull a bogus id from an empty store 200× and assert every .get() re-throws
// a stable-coded snapdir::Error — valgrind (cpp-quality) runs this loop and a
// per-throw async leak would be unmistakable.
static void test_async_raii_frees_on_throw(const fs::path &root) {
    const std::string empty_store =
        "file://" + (root / "async-empty-store").string();
    const std::string bogus_id(64, '0');

    int thrown = 0;
    for (int i = 0; i < 200; ++i) {
        const fs::path bad_dest = root / ("async-bad-dest-" + std::to_string(i));
        fs::create_directories(bad_dest);
        std::future<void> f = snapdir::pull(bogus_id, empty_store, bad_dest);
        bool caught = false;
        try {
            f.get();
        } catch (const snapdir::Error &e) {
            caught = true;
            ++thrown;
            CHECK(std::strlen(e.what()) > 0,
                  "async Error::what() must be non-empty on the .get() re-throw path");
            CHECK(is_stable_code(e.code()),
                  "async pull(bogus).get() code must be a stable ABI code (no UAF)");
        } catch (...) {
            CHECK(false, "async pull(bogus).get() threw a non-snapdir::Error type");
        }
        CHECK(caught, "every async pull(bogus).get() MUST re-throw snapdir::Error");
    }
    CHECK(thrown == 200, "every iteration of the async throw loop must have thrown");
}

// ─── 8. std::future exception propagation for push/fetch/diff (strengthening) ─
//
// test_async_roundtrip pins the FAILING-pull re-throw. The other three async
// entry points (push, fetch, diff) must ALSO propagate snapdir::Error from
// .get() with a stable .code() — each has independent ErrorGuard wiring in its
// own std::async lambda, so a guard bug in one is invisible to the pull test.
//   - push to an INVALID store URI → INVALID_STORE (the ABI validates the URI).
//   - fetch a bogus id from an empty store → stable code (IO/STORE/INVALID_ID).
//   - diff against an INVALID store URI → INVALID_STORE.
static void test_future_exception_propagation(const fs::path &tree,
                                              const fs::path &root) {
    // push() → invalid store URI re-throws with INVALID_STORE.
    {
        bool threw = false;
        try {
            snapdir::push(tree, "not-a-valid-uri").get();
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(is_stable_code(e.code()),
                  "push(invalid store).get() code must be a stable ABI code");
            CHECK(e.code() == "INVALID_STORE",
                  "push to a syntactically-invalid store URI must be INVALID_STORE");
        } catch (...) {
            CHECK(false, "push(invalid store).get() threw a non-snapdir::Error type");
        }
        CHECK(threw, "push() to an invalid store must re-throw from .get()");
    }

    // fetch() → bogus id from an empty store re-throws with a stable code.
    {
        const std::string empty_store =
            "file://" + (root / "fetch-empty-store").string();
        const std::string bogus_id(64, '0');
        bool threw = false;
        try {
            snapdir::fetch(bogus_id, empty_store).get();
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(is_stable_code(e.code()),
                  "fetch(bogus).get() code must be a stable ABI code");
        } catch (...) {
            CHECK(false, "fetch(bogus).get() threw a non-snapdir::Error type");
        }
        CHECK(threw, "fetch() of a bogus id from an empty store must re-throw from .get()");
    }

    // diff() → invalid store URI re-throws with INVALID_STORE.
    {
        const std::string good_store =
            "file://" + (root / "diff-prop-store").string();
        // Stage a real store on one side so the failure is the URI, not emptiness.
        try {
            snapdir::push(tree, good_store).get();
        } catch (const snapdir::Error &e) {
            CHECK(false, (std::string("diff-prop push threw: ") + e.what()).c_str());
            return;
        }
        bool threw = false;
        try {
            snapdir::diff(good_store, "ftp://nope").get();
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(is_stable_code(e.code()),
                  "diff(invalid store).get() code must be a stable ABI code");
            CHECK(e.code() == "INVALID_STORE",
                  "diff against a non-file/invalid store URI must be INVALID_STORE");
        } catch (...) {
            CHECK(false, "diff(invalid store).get() threw a non-snapdir::Error type");
        }
        CHECK(threw, "diff() against an invalid store must re-throw from .get()");
    }
}

// ─── 9. Error.code() exactness where the ABI is deterministic (strengthening) ─
//
// test_error_code_is_stable only pins MEMBERSHIP for the missing-path case
// (the ABI does not guarantee IO_ERROR vs another code there). But two inputs
// ARE deterministic at the ABI: a syntactically-invalid store URI → INVALID_STORE,
// and a malformed snapshot id → INVALID_ID. We pin the EXACT code, not just
// membership, so a regression that swapped the mapping would be caught.
static void test_error_code_exactness(const fs::path &tree, const fs::path &root) {
    // Invalid store URI on a sync-reachable path: push().get() (async) already
    // covered above; here pin pull() too — a bogus store scheme is INVALID_STORE.
    {
        const fs::path dest = root / "exact-dest";
        fs::create_directories(dest);
        const std::string valid_id(64, 'a'); // well-formed 64-hex, just absent
        bool threw = false;
        try {
            snapdir::pull(valid_id, "://broken", dest).get();
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(e.code() == "INVALID_STORE",
                  "pull from a malformed store URI must be exactly INVALID_STORE");
        } catch (...) {
            CHECK(false, "pull(malformed store).get() threw a non-snapdir::Error type");
        }
        CHECK(threw, "pull() from a malformed store URI must re-throw");
    }

    // Malformed snapshot id (not 64-hex) against a real store → INVALID_ID.
    {
        const std::string store =
            "file://" + (root / "exact-store").string();
        try {
            snapdir::push(tree, store).get(); // make the store real
        } catch (const snapdir::Error &e) {
            CHECK(false, (std::string("exact-store push threw: ") + e.what()).c_str());
            return;
        }
        const fs::path dest = root / "exact-dest2";
        fs::create_directories(dest);
        bool threw = false;
        try {
            // "xyz" is not a 64-hex id — the ABI must reject it as INVALID_ID
            // BEFORE any store I/O.
            snapdir::pull("xyz-not-a-valid-id", store, dest).get();
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(e.code() == "INVALID_ID",
                  "pull with a malformed snapshot id must be exactly INVALID_ID");
        } catch (...) {
            CHECK(false, "pull(malformed id).get() threw a non-snapdir::Error type");
        }
        CHECK(threw, "pull() with a malformed snapshot id must re-throw");
    }
}

// ─── 10. std::filesystem::path edge cases — id stability (strengthening) ──────
//
// snapdir.hpp parses manifest TEXT into ManifestEntry.path (fs::path) and feeds
// fs::path::c_str() back into the C ABI. Awkward path components — a space, a
// unicode name, a deeply-nested dir, and an EMPTY directory — must round-trip
// through manifest()/id()/id_from_manifest() with a STABLE 64-hex id and the
// entries must be enumerated (the path is not truncated at the space, the empty
// dir is recorded). This pins fs::path handling end-to-end through the binding.
static void test_filesystem_path_edge_cases(const fs::path &root) {
    const fs::path tree = root / "fs-edge-tree";
    // Use plain narrow (UTF-8 byte) literals — NOT u8"" (char8_t in C++20, which
    // does not implicitly convert and trips -Werror under both compilers). The
    // source file is UTF-8 so these bytes are the literal UTF-8 sequence; the FS
    // and the C ABI treat paths as opaque bytes, so the unicode name round-trips.
    const std::string uni_dir = "unic\xC3\xB3""de-d\xC3\xAD""r"; // "unicóde-dír"
    const std::string uni_file = "f\xC3\xAD""le.txt";            // "fíle.txt"
    fs::create_directories(tree / "a b" / "nested deep");          // spaces, nesting
    fs::create_directories(tree / uni_dir);                        // unicode dir
    fs::create_directories(tree / "empty-dir");                    // empty dir
    { std::ofstream(tree / "a b" / "file with spaces.txt") << "x"; }
    { std::ofstream(tree / "a b" / "nested deep" / "leaf.bin") << "yy"; }
    { std::ofstream(tree / uni_dir / uni_file) << "z"; }

    const std::string id1 = snapdir::id(tree);
    CHECK(is_hex64(id1), "id() over a tree with spaces/unicode/empty-dir must be 64-hex");
    const std::string id2 = snapdir::id(tree);
    CHECK(id1 == id2, "id() over the awkward-path tree must be deterministic");

    snapdir::Manifest m = snapdir::manifest(tree);
    CHECK(!m.entries.empty(), "awkward-path manifest must have entries");
    CHECK(snapdir::id_from_manifest(m) == id1,
          "id_from_manifest must equal id() for the awkward-path tree (path round-trip)");

    // The space-containing file must be parsed with its FULL name (the manifest
    // parser must not truncate the path at the first space). Match the leaf.
    auto has_leaf = [&](const std::string &leaf) {
        for (const auto &e : m.entries) {
            if (e.path.filename().string() == leaf) return true;
        }
        return false;
    };
    CHECK(has_leaf("file with spaces.txt"),
          "manifest must record the full path of a file whose name contains spaces");
    CHECK(has_leaf("leaf.bin"), "manifest must record the deeply-nested leaf");

    // The empty directory must appear as a Directory entry (not dropped). The
    // manifest records directories with a TRAILING slash (e.g. "./empty-dir/"),
    // so fs::path::filename() is empty — normalize (strip leading "./" + trailing
    // "/") before matching, exactly like test #3's `norm`.
    auto norm = [](const fs::path &p) {
        std::string s = p.generic_string();
        if (s.rfind("./", 0) == 0) s.erase(0, 2);
        while (!s.empty() && s.back() == '/') s.pop_back();
        return s;
    };
    bool saw_empty_dir = false;
    for (const auto &e : m.entries) {
        if (norm(e.path) == "empty-dir" &&
            e.type == snapdir::PathType::Directory) {
            saw_empty_dir = true;
        }
    }
    CHECK(saw_empty_dir, "manifest must record the empty directory as a D entry");
}

// ─── 11. diff-JSON parser pairs status↔path PER OBJECT (strengthening) ────────
//
// The impl's parse_diff_json (snapdir.hpp) was hardened in cpp-api-impl to parse
// each JSON object independently (status + path from the SAME object) and to skip
// escaped quotes. A regression that, say, scanned for the first "status" and the
// first "path" across the WHOLE array would cross-couple a status from object N
// with a path from object M. We pin per-object pairing through a REAL diff with
// MULTIPLE change rows (added c.txt, deleted gone.txt, modified a.txt) and assert
// each glyph is paired with ITS OWN path — not just that the set of glyphs and the
// set of paths each appear somewhere. A path with a SPACE ("a b.txt") additionally
// exercises the string-value scan (the parser must not stop at whitespace).
static void test_diff_json_per_object_pairing(const fs::path &root) {
    const std::string storeA = "file://" + (root / "pair-storeA").string();
    const std::string storeB = "file://" + (root / "pair-storeB").string();

    // A: a.txt (mod base), gone.txt (will be deleted), keep.txt (unchanged),
    //    "a b.txt" (space in name, unchanged) .
    const fs::path treeA = root / "pair-treeA";
    fs::create_directories(treeA);
    { std::ofstream(treeA / "a.txt")    << "base"; }
    { std::ofstream(treeA / "gone.txt") << "remove-me"; }
    { std::ofstream(treeA / "keep.txt") << "same"; }
    { std::ofstream(treeA / "a b.txt")  << "spaced"; }

    // B: a.txt modified, gone.txt removed, keep.txt + "a b.txt" unchanged,
    //    c.txt added.
    const fs::path treeB = root / "pair-treeB";
    fs::create_directories(treeB);
    { std::ofstream(treeB / "a.txt")    << "CHANGED-bytes"; }
    { std::ofstream(treeB / "keep.txt") << "same"; }
    { std::ofstream(treeB / "a b.txt")  << "spaced"; }
    { std::ofstream(treeB / "c.txt")    << "brand-new"; }

    try {
        snapdir::push(treeA, storeA).get();
        snapdir::push(treeB, storeB).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("pairing push threw: ") + e.what()).c_str());
        return;
    }

    std::vector<snapdir::DiffEntry> entries;
    try {
        entries = snapdir::diff(storeA, storeB).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("pairing diff threw: ") + e.what()).c_str());
        return;
    }
    CHECK(entries.size() >= 3, "multi-change diff must report at least A/D/M rows");

    // Per-object pairing: each (status, path) pair must be correct TOGETHER. If
    // the parser cross-coupled fields, a.txt would not consistently be Modified,
    // c.txt would not be Added, gone.txt would not be Deleted.
    int matched = 0;
    for (const auto &e : entries) {
        const std::string name = e.path.filename().string();
        if (name == "a.txt") {
            CHECK(e.status == snapdir::DiffStatus::Modified,
                  "a.txt must pair with Modified (per-object status↔path)");
            ++matched;
        } else if (name == "c.txt") {
            CHECK(e.status == snapdir::DiffStatus::Added,
                  "c.txt must pair with Added (per-object status↔path)");
            ++matched;
        } else if (name == "gone.txt") {
            CHECK(e.status == snapdir::DiffStatus::Deleted,
                  "gone.txt must pair with Deleted (per-object status↔path)");
            ++matched;
        }
        // No entry may carry an empty path — the parser must always decode the
        // path value from the same object as the status.
        CHECK(!e.path.empty(),
              "every DiffEntry must have a non-empty path (status↔path same object)");
    }
    CHECK(matched == 3,
          "diff must pair each of a.txt/c.txt/gone.txt with its own correct status");
}

// ─── version() sanity ────────────────────────────────────────────────────────
static void test_version() {
    const std::string v = snapdir::version();
    CHECK(!v.empty(), "version() must be non-empty");
}

// ─── size tests (Phase 46, folded from .gatesmith/pending-tests/cpp_size.cpp) ─
//
// Deterministic fixture (mirrors the Rust api test for cross-binding parity):
//   seed/a/x.txt   = "hello world\n"      (12 bytes)
//   seed/b/dup.txt = "hello world\n"      (12 bytes — DUPLICATE content of x.txt)
//   seed/a/y.txt   = "unique data here\n" (17 bytes)
// ⇒ files=3, objects=2, logical_bytes=41 (12+12+17), physical_bytes=29 (12+17).
static const std::uint64_t kExpFiles    = 3;
static const std::uint64_t kExpObjects  = 2;
static const std::uint64_t kExpLogical  = 41;
static const std::uint64_t kExpPhysical = 29;

// The seed's shared 12-byte content; snapshot B (below) reuses it verbatim so
// its checksum collides with seed/a/x.txt — the lever for cross-snapshot dedup.
static const std::string kSharedContent = "hello world\n";         // 12 bytes
// A content object that exists ONLY in snapshot B (a new distinct checksum).
static const std::string kBFreshContent = "second snapshot only\n";

static fs::path build_seed_tree(const fs::path &seed) {
    fs::create_directories(seed / "a");
    fs::create_directories(seed / "b");
    { std::ofstream(seed / "a" / "x.txt")   << "hello world\n"; }      // 12 bytes
    { std::ofstream(seed / "b" / "dup.txt") << "hello world\n"; }      // 12 (dup)
    { std::ofstream(seed / "a" / "y.txt")   << "unique data here\n"; } // 17 bytes
    ::chmod((seed / "a" / "x.txt").c_str(),   0644);
    ::chmod((seed / "b" / "dup.txt").c_str(), 0644);
    ::chmod((seed / "a" / "y.txt").c_str(),   0644);
    return seed;
}

// Snapshot B: shares ONE content object with the seed (kSharedContent) and adds
// ONE brand-new object (kBFreshContent). Pushing seed + seed2 into the same
// store exercises whole-store dedup ACROSS snapshots, not just within one.
static fs::path build_seed2_tree(const fs::path &seed2) {
    fs::create_directories(seed2);
    { std::ofstream(seed2 / "reuse.txt") << kSharedContent; } // dup of seed's hello-world object
    { std::ofstream(seed2 / "fresh.txt") << kBFreshContent; } // a new, unique object
    ::chmod((seed2 / "reuse.txt").c_str(), 0644);
    ::chmod((seed2 / "fresh.txt").c_str(), 0644);
    return seed2;
}

// Recursive on-disk byte total of a directory subtree (regular files only).
static std::uint64_t on_disk_bytes(const fs::path &dir) {
    std::uint64_t total = 0;
    std::error_code ec;
    if (!fs::exists(dir, ec)) return 0;
    for (auto it = fs::recursive_directory_iterator(
             dir, fs::directory_options::skip_permission_denied, ec);
         !ec && it != fs::recursive_directory_iterator(); it.increment(ec)) {
        std::error_code fec;
        if (it->is_regular_file(fec) && !fec) {
            const auto sz = it->file_size(fec);
            if (!fec) total += static_cast<std::uint64_t>(sz);
        }
    }
    return total;
}

// The filesystem path a "file://<path>" store URI points at.
static fs::path store_path_of(const std::string &store_uri) {
    const std::string prefix = "file://";
    return fs::path(store_uri.substr(prefix.size()));
}

// ─── 1. manifest_size on a LOCAL manifest pins dedup-by-checksum ─────────────
static void test_manifest_size(const fs::path &seed) {
    snapdir::Manifest m = snapdir::manifest(seed);
    CHECK(!m.entries.empty(), "seed manifest must have entries");

    snapdir::SizeStats s = snapdir::manifest_size(m);
    CHECK(s.files == kExpFiles,          "manifest_size.files must be 3 (File entries)");            // clause: files
    CHECK(s.objects == kExpObjects,      "manifest_size.objects must be 2 (distinct checksums)");    // clause: objects
    CHECK(s.logical_bytes == kExpLogical,"manifest_size.logical_bytes must be 41 (dups counted)");   // clause: logical_bytes
    CHECK(s.physical_bytes == kExpPhysical,
          "manifest_size.physical_bytes must be 29 (deduped by checksum)");                          // clause: physical_bytes
}

// ─── 2. size(store, id) of a single pushed snapshot == the local figure ──────
static void test_size_single_snapshot(const fs::path &seed, const fs::path &root) {
    const std::string store =
        "file://" + (root / "store-single").string();

    std::string id;
    try {
        id = snapdir::push(seed, store).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push(seed) threw: ") + e.what()).c_str());
        return;
    }
    CHECK(is_hex64(id), "push id must be 64-hex");

    snapdir::SizeStats s;
    try {
        s = snapdir::size(store, std::optional<std::string>(id)).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("size(store,id).get() threw: ") + e.what()).c_str());
        return;
    }
    CHECK(s.files == kExpFiles,           "size(store,id).files must be 3");                 // clause: single-snapshot == manifest_size
    CHECK(s.objects == kExpObjects,       "size(store,id).objects must be 2");
    CHECK(s.logical_bytes == kExpLogical, "size(store,id).logical_bytes must be 41");
    CHECK(s.physical_bytes == kExpPhysical,
          "size(store,id).physical_bytes must be 29");
}

// ─── 3. size(store, nullopt) folds the WHOLE store ───────────────────────────
static void test_size_whole_store_nullopt(const fs::path &seed, const fs::path &root) {
    const std::string store =
        "file://" + (root / "store-whole").string();

    std::string id;
    try {
        id = snapdir::push(seed, store).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push(seed) threw: ") + e.what()).c_str());
        return;
    }

    snapdir::SizeStats by_id, whole;
    try {
        by_id = snapdir::size(store, std::optional<std::string>(id)).get();
        whole = snapdir::size(store, std::nullopt).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("size() threw: ") + e.what()).c_str());
        return;
    }
    CHECK(whole.files == by_id.files,                 "nullopt fold files must equal single-snapshot files");        // clause: nullopt = whole-store fold
    CHECK(whole.objects == by_id.objects,             "nullopt fold objects must equal single-snapshot objects");
    CHECK(whole.logical_bytes == by_id.logical_bytes, "nullopt fold logical must equal single-snapshot logical");
    CHECK(whole.physical_bytes == by_id.physical_bytes,
          "nullopt fold physical must equal single-snapshot physical");
    // And the absolute figure, to pin it to the fixture, not just to itself.
    CHECK(whole.physical_bytes == kExpPhysical, "whole-store physical_bytes must be 29");
}

// ─── 4. ACCEPTANCE: physical_bytes == on-disk .objects/ bytes ────────────────
static void test_physical_equals_on_disk_objects(const fs::path &seed, const fs::path &root) {
    const std::string store =
        "file://" + (root / "store-acceptance").string();

    try {
        snapdir::push(seed, store).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push(seed) threw: ") + e.what()).c_str());
        return;
    }

    snapdir::SizeStats s;
    try {
        s = snapdir::size(store, std::nullopt).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("size(store,nullopt).get() threw: ") + e.what()).c_str());
        return;
    }

    const fs::path objects_dir = store_path_of(store) / ".objects";
    CHECK(fs::exists(objects_dir), "file:// store must have a .objects/ directory after push");
    const std::uint64_t disk = on_disk_bytes(objects_dir);
    CHECK(disk > 0, "on-disk .objects/ byte total must be > 0");
    CHECK(s.physical_bytes == disk,
          "physical_bytes MUST equal the recursive on-disk .objects/ byte total (uncompressed)"); // clause: ACCEPTANCE
    // And that on-disk total is the fixture's deduped 29 bytes.
    CHECK(disk == kExpPhysical, "on-disk .objects/ total must be 29 (deduped, uncompressed)");
}

// ─── 5. dedup actually happened: logical > physical, objects < files ─────────
static void test_dedup_strict_inequalities(const fs::path &seed) {
    snapdir::SizeStats s = snapdir::manifest_size(snapdir::manifest(seed));
    CHECK(s.logical_bytes > s.physical_bytes,
          "logical_bytes must be STRICTLY greater than physical_bytes (dedup happened)"); // clause: dedup strict
    CHECK(s.objects < s.files,
          "objects must be STRICTLY fewer than files (a duplicate collapsed)");           // clause: objects < files
}

// ─── 6. throw-on-bad-store: size() of an unopenable store re-throws ──────────
static void test_size_bad_store_throws() {
    bool threw = false;
    try {
        snapdir::SizeStats s =
            snapdir::size("file:///nonexistent/definitely/missing", std::nullopt).get();
        (void)s; // unreachable on the contract
    } catch (const snapdir::Error &e) {
        threw = true;
        CHECK(std::strlen(e.what()) > 0, "size(bad).what() must be non-empty on throw");
        CHECK(is_stable_code(e.code()),  "size(bad).code() must be a stable ABI code");   // clause: throw-on-bad-store
    } catch (...) {
        CHECK(false, "size(bad store).get() threw a non-snapdir::Error type");
    }
    CHECK(threw, "size() of an unopenable store MUST re-throw snapdir::Error from .get()");
}

// ─── 7. edge: empty/degenerate manifest ⇒ all-zero stats, no throw ───────────
static void test_empty_manifest_all_zeros(const fs::path &root) {
    const fs::path empty = root / "empty-seed";
    fs::create_directories(empty); // a directory tree with zero files
    snapdir::Manifest m = snapdir::manifest(empty);
    snapdir::SizeStats s = snapdir::manifest_size(m);
    CHECK(s.files == 0,          "empty tree ⇒ files == 0");           // clause: empty ⇒ all-zeros
    CHECK(s.objects == 0,        "empty tree ⇒ objects == 0");
    CHECK(s.logical_bytes == 0,  "empty tree ⇒ logical_bytes == 0");
    CHECK(s.physical_bytes == 0, "empty tree ⇒ physical_bytes == 0");
}

// ─── 8. future/RAII no-leak on the throw path (ASan/UBSan bar) ───────────────
static void test_size_future_throw_no_leak() {
    int thrown = 0;
    for (int i = 0; i < 100; ++i) {
        bool caught = false;
        try {
            snapdir::SizeStats s =
                snapdir::size("file:///no/such/store/for/size/leak", std::nullopt).get();
            (void)s;
        } catch (const snapdir::Error &e) {
            caught = true;
            ++thrown;
            CHECK(std::strlen(e.what()) > 0,
                  "async size Error::what() must be non-empty on the .get() re-throw");
            CHECK(is_stable_code(e.code()),
                  "async size(bad).get() code must be a stable ABI code (no UAF)");
        } catch (...) {
            CHECK(false, "async size(bad).get() threw a non-snapdir::Error type");
        }
        CHECK(caught, "every size(bad).get() MUST re-throw snapdir::Error");
    }
    CHECK(thrown == 100, "every iteration of the size throw loop must have thrown"); // clause: RAII/ASan-clean
}

// ─── 9. (tests-review) directories are present in the manifest but NOT counted ─
//
// Impl-revealed: manifest_size does `if (e.type != PathType::File) continue;`.
// The seed has subdirectories a/ and b/, so its manifest MUST carry Directory
// entries — that is exactly what exercises the non-File skip branch. snapdir
// directory entries are NOT inert: they carry a real Merkle subtree checksum
// and a CUMULATIVE subtree size (e.g. "./" size 41). That is precisely why the
// skip is load-bearing: were directories counted, logical would balloon to
// 41 + Σ(dir subtree sizes) and objects would count the directory hashes. Pin
// that these substantial directory entries are present yet contribute nothing.
static void test_manifest_size_directory_excluded(const fs::path &seed) {
    snapdir::Manifest m = snapdir::manifest(seed);
    std::uint64_t dir_entries = 0, dir_size_sum = 0;
    bool saw_dir_with_content = false;
    for (const auto &e : m.entries) {
        if (e.type == snapdir::PathType::Directory) {
            ++dir_entries;
            dir_size_sum += e.size;
            // A directory entry carries a real subtree hash + cumulative size,
            // so it WOULD corrupt the count if the != File skip were removed.
            if (!e.checksum.empty() && e.size > 0) saw_dir_with_content = true;
        }
    }
    CHECK(dir_entries > 0, "seed manifest must contain at least one Directory entry (a/, b/)");      // clause: dirs present
    CHECK(saw_dir_with_content,
          "a Directory entry must carry a non-empty subtree checksum and non-zero size");            // clause: dirs look like content
    snapdir::SizeStats s = snapdir::manifest_size(m);
    CHECK(s.files == kExpFiles,     "directories must NOT count toward files");                      // clause: dirs excluded (files)
    CHECK(s.objects == kExpObjects, "directories must NOT count toward objects");                    // clause: dirs excluded (objects)
    // The decisive byte check: logical stays 41 and does NOT absorb the sizeable
    // directory subtree sizes — proving the skip excludes directory bytes.
    CHECK(dir_size_sum > 0, "directory subtree sizes must be non-zero (skip is meaningful)");
    CHECK(s.logical_bytes == kExpLogical,
          "logical_bytes must stay 41 (directory subtree sizes must NOT be added)");                 // clause: dir bytes excluded
    CHECK(s.logical_bytes < kExpLogical + dir_size_sum,
          "logical must be STRICTLY less than files+dirs size sum (directories dropped)");           // clause: dirs strictly dropped
    CHECK(m.entries.size() > s.files,
          "manifest must hold MORE entries than files (directory entries present, uncounted)");      // clause: entries > files
}

// ─── 10. (tests-review) TWO-snapshot store: dedup ACROSS snapshots + id select ─
//
// Push snapshot A (seed) and snapshot B (seed2 — shares one object with A, adds
// one new object) into the SAME store. Pins: (a) per-id selection — size(store,
// idA) is A's own figure, size(store, idB) is B's, NOT the whole store; (b) the
// whole-store fold unions BOTH manifests and dedups the shared object ACROSS
// snapshots (physical counts the shared 12 bytes ONCE); (c) whole physical ==
// on-disk .objects/ across snapshots; (d) re-run determinism.
static void test_two_snapshot_cross_dedup(const fs::path &seed, const fs::path &root) {
    const fs::path   seed2 = build_seed2_tree(root / "seed2");
    const std::string store = "file://" + (root / "store-multi").string();

    std::string idA, idB;
    try {
        idA = snapdir::push(seed,  store).get();
        idB = snapdir::push(seed2, store).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("push(A/B) threw: ") + e.what()).c_str());
        return;
    }
    CHECK(is_hex64(idA) && is_hex64(idB), "both snapshot ids must be 64-hex");
    CHECK(idA != idB, "two distinct trees must yield two distinct snapshot ids");                // clause: distinct ids

    const std::uint64_t lenShared = static_cast<std::uint64_t>(kSharedContent.size()); // 12
    const std::uint64_t lenFresh  = static_cast<std::uint64_t>(kBFreshContent.size());
    // Snapshot B alone: 2 files, 2 distinct checksums within B.
    const std::uint64_t expFilesB    = 2;
    const std::uint64_t expObjectsB  = 2;
    const std::uint64_t expLogicalB  = lenShared + lenFresh;
    const std::uint64_t expPhysicalB = lenShared + lenFresh;

    snapdir::SizeStats sA, sB, whole, whole2;
    try {
        sA     = snapdir::size(store, std::optional<std::string>(idA)).get();
        sB     = snapdir::size(store, std::optional<std::string>(idB)).get();
        whole  = snapdir::size(store, std::nullopt).get();
        whole2 = snapdir::size(store, std::nullopt).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("size() threw: ") + e.what()).c_str());
        return;
    }

    // (a) per-id selection: idA yields A's own figure, NOT the union.
    CHECK(sA.files == kExpFiles && sA.objects == kExpObjects &&
          sA.logical_bytes == kExpLogical && sA.physical_bytes == kExpPhysical,
          "size(store,idA) must equal snapshot A's own size, not the whole store");              // clause: id A selects A
    CHECK(sB.files == expFilesB,            "size(store,idB).files must be 2 (B's own entries)"); // clause: id B selects B
    CHECK(sB.objects == expObjectsB,        "size(store,idB).objects must be 2 (B's checksums)");
    CHECK(sB.logical_bytes == expLogicalB,  "size(store,idB).logical must be reuse+fresh bytes");
    CHECK(sB.physical_bytes == expPhysicalB,"size(store,idB).physical must be reuse+fresh bytes");

    // (b) whole-store fold unions BOTH manifests, deduped by checksum across them.
    const std::uint64_t expFilesWhole    = kExpFiles + expFilesB;     // 5 File entries total
    const std::uint64_t expObjectsWhole  = 3;                         // {hello, unique, fresh}
    const std::uint64_t expLogicalWhole  = kExpLogical + expLogicalB; // dups counted across snapshots
    const std::uint64_t expPhysicalWhole = kExpPhysical + lenFresh;   // hello counted ONCE across snapshots

    CHECK(whole.files == expFilesWhole,   "whole-store files must sum File entries across BOTH manifests"); // clause: union files
    CHECK(whole.objects == expObjectsWhole,"whole-store objects must be 3 distinct checksums across both");  // clause: cross-snap distinct objects
    CHECK(whole.logical_bytes == expLogicalWhole,
          "whole-store logical must be ΣA + ΣB logical (dups counted across snapshots)");                    // clause: union logical
    CHECK(whole.physical_bytes == expPhysicalWhole,
          "whole-store physical must dedup the shared object ACROSS snapshots (hello once)");                // clause: cross-snap dedup

    // Cross-snapshot dedup is STRICTLY observable: shared object not doubled.
    CHECK(whole.physical_bytes < sA.physical_bytes + sB.physical_bytes,
          "whole physical must be STRICTLY < Σper-snapshot physical (shared object deduped)");               // clause: cross-snap dedup strict

    // (c) ACCEPTANCE across snapshots: whole physical == on-disk .objects/ bytes.
    const fs::path objects_dir = store_path_of(store) / ".objects";
    const std::uint64_t disk = on_disk_bytes(objects_dir);
    CHECK(disk == expPhysicalWhole,
          "on-disk .objects/ total must equal the cross-snapshot deduped physical bytes");                  // clause: multi-snap acceptance
    CHECK(whole.physical_bytes == disk,
          "whole-store physical_bytes MUST equal the recursive on-disk .objects/ byte total");

    // (d) determinism: a repeated whole-store fold yields identical stats.
    CHECK(whole.files == whole2.files && whole.objects == whole2.objects &&
          whole.logical_bytes == whole2.logical_bytes &&
          whole.physical_bytes == whole2.physical_bytes,
          "size(store,nullopt) must be deterministic across repeated calls");                               // clause: determinism
}

// ─── 11. (tests-review) EMPTY store ⇒ all-zeros, NO throw ────────────────────
//
// An EXISTING store directory with no snapshots pushed. The whole-store fold
// must return all-zero stats and MUST NOT throw — this is the legitimate-empty
// branch, distinct from the missing/unopenable store that DOES throw (test 6).
static void test_size_empty_store_all_zeros(const fs::path &root) {
    const fs::path empty_store_dir = root / "empty-store";
    fs::create_directories(empty_store_dir);
    const std::string store = "file://" + empty_store_dir.string();

    snapdir::SizeStats s{1, 1, 1, 1}; // poison; must be overwritten to zeros
    try {
        s = snapdir::size(store, std::nullopt).get();
    } catch (const snapdir::Error &e) {
        CHECK(false, (std::string("size(empty store,nullopt) threw: ") + e.what()).c_str());
        return;
    }
    CHECK(s.files == 0 && s.objects == 0 &&
          s.logical_bytes == 0 && s.physical_bytes == 0,
          "an existing store with no snapshots must fold to all-zero stats, no throw"); // clause: empty store zeros
}

// ─── 12. (tests-review) malformed / unsupported store URIs still throw ───────
//
// Beyond the missing file:// path (test 6): a URI with no scheme and a URI with
// a valid but unsupported scheme must BOTH throw snapdir::Error with a stable
// code — never return bogus zero stats.
static void test_size_bad_uri_variants_throw() {
    const char *bad_uris[] = {
        "not-a-store-uri-at-all", // no scheme / no "://"
        "bogus://whatever/store", // well-formed but unsupported adapter
    };
    for (const char *uri : bad_uris) {
        bool threw = false;
        try {
            snapdir::SizeStats s = snapdir::size(uri, std::nullopt).get();
            (void)s;
        } catch (const snapdir::Error &e) {
            threw = true;
            CHECK(std::strlen(e.what()) > 0, "bad-URI size().what() must be non-empty");
            CHECK(is_stable_code(e.code()),  "bad-URI size().code() must be a stable ABI code"); // clause: bad-URI throws
        } catch (...) {
            CHECK(false, "size(bad URI) threw a non-snapdir::Error type");
        }
        CHECK(threw, "size() of a malformed/unsupported store URI MUST throw snapdir::Error");
    }
}

int main() {
    // Hermetic + offline: route the cache + catalog into a private temp root so
    // the test never touches the real cache and never hits the network.
    fs::path root = make_tmp_root();
    const fs::path cache   = root / "cache";
    const fs::path catalog = root / "catalog.db";
    fs::create_directories(cache);
    ::setenv("SNAPDIR_CACHE_DIR", cache.c_str(), 1);
    ::setenv("SNAPDIR_CATALOG_DB_PATH", catalog.c_str(), 1);

    const fs::path tree = build_tree(root / "tree");
    const fs::path seed = build_seed_tree(root / "seed");

    // The paramount axis first, then behaviour.
    test_version();
    test_raii_frees_on_throw();
    test_error_code_is_stable();
    test_manifest_and_id_self_consistency(tree);
    test_options_honoured(tree);
    test_async_roundtrip(tree, root);
    test_diff_across_stores(tree, root);
    // strengthening (tests-review): async throw-path leak, future propagation
    // for push/fetch/diff, exact error codes, fs::path edge cases, and per-object
    // diff-JSON pairing.
    test_async_raii_frees_on_throw(root);
    test_future_exception_propagation(tree, root);
    test_error_code_exactness(tree, root);
    test_filesystem_path_edge_cases(root);
    test_diff_json_per_object_pairing(root);
    // Phase 46: size surface (folded from .gatesmith/pending-tests/cpp_size.cpp)
    test_manifest_size(seed);                         // 1
    test_size_single_snapshot(seed, root);            // 2
    test_size_whole_store_nullopt(seed, root);        // 3
    test_physical_equals_on_disk_objects(seed, root); // 4 (ACCEPTANCE)
    test_dedup_strict_inequalities(seed);             // 5
    test_size_bad_store_throws();                     // 6
    test_empty_manifest_all_zeros(root);              // 7
    test_size_future_throw_no_leak();                 // 8
    // Phase-46 tests-review hardening (impl-revealed branches):
    test_manifest_size_directory_excluded(seed);      // 9  directories excluded
    test_two_snapshot_cross_dedup(seed, root);        // 10 cross-snapshot dedup + per-id + determinism
    test_size_empty_store_all_zeros(root);            // 11 empty store ⇒ zeros, no throw
    test_size_bad_uri_variants_throw();               // 12 malformed/unsupported URIs throw

    // Clean up the private temp root (best-effort; valgrind cares about heap,
    // not the FS).
    std::error_code ec;
    fs::remove_all(root, ec);

    std::fprintf(stderr, "checks: %d, failures: %d\n", g_checks, g_failures);
    return g_failures == 0 ? 0 : 1;
}
