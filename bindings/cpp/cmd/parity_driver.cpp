// parity_driver.cpp — C++-binding driver for the cross-language parity harness
// (tests/golden/run_parity.sh, §1 protocol). It exercises the public `snapdir`
// C++ RAII binding (snapdir.hpp over the C ABI) and emits BYTE-EXACT stdout:
//
//   parity-driver manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
//   parity-driver id       <path> [--no-follow] [--absolute] [--exclude <RE>]...
//   parity-driver push     <path> <store_uri> [--jobs N]
//   parity-driver fetch    <id>   <store_uri>
//   parity-driver checkout <id>   <store_uri> <dest>
//
// stdout is byte-exact per the spec; diagnostics go to stderr; exit 0 = success.
// The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
// SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
// (§1.6); the C++ binding wraps the C ABI → snapdir-api which honors those — env
// inherited. Build-tool: native clang++/g++ (the image has no cmake); the gate
// compiles this to tests/golden/drivers/cpp-driver-bin before the harness runs.
//
// LANE NOTE: this + tests/golden/drivers/cpp.sh only CONSUME the binding (the
// public snapdir:: surface) — never reimplement walk/hash/store logic.

#include <snapdir.hpp>

#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

[[noreturn]] static void die(const std::string &msg) {
    std::fprintf(stderr, "[parity-driver] %s\n", msg.c_str());
    std::exit(1);
}

// combine_excludes mirrors the Go binding's excludePattern: 0 → "" (no exclude),
// 1 → the pattern as-is, N → `(?:p1)|(?:p2)|…` (OR-combined alternation), so that
// repeated --exclude flags reduce to the single regex the C ABI accepts.
static std::string combine_excludes(const std::vector<std::string> &pats) {
    if (pats.empty()) {
        return "";
    }
    if (pats.size() == 1) {
        return pats[0];
    }
    std::string out;
    for (std::size_t i = 0; i < pats.size(); ++i) {
        if (i != 0) {
            out += "|";
        }
        out += "(?:" + pats[i] + ")";
    }
    return out;
}

// parse_path_and_opts parses `<path> [--no-follow] [--absolute] [--exclude <RE>]…`
// into the path and native snapdir::ManifestOptions.
static std::string parse_path_and_opts(const std::vector<std::string> &args,
                                       snapdir::ManifestOptions &opts) {
    std::string path;
    std::vector<std::string> excludes;
    const std::string kExcludeEq = "--exclude=";
    for (std::size_t i = 0; i < args.size(); ++i) {
        const std::string &a = args[i];
        if (a == "--no-follow") {
            opts.no_follow = true;
        } else if (a == "--absolute") {
            opts.absolute = true;
        } else if (a == "--exclude") {
            if (++i >= args.size()) {
                die("--exclude requires an argument");
            }
            excludes.push_back(args[i]);
        } else if (a.rfind(kExcludeEq, 0) == 0) {
            excludes.push_back(a.substr(kExcludeEq.size()));
        } else if (!a.empty() && a[0] == '-') {
            die("unknown flag " + a);
        } else if (path.empty()) {
            path = a;
        } else {
            die("unexpected extra argument " + a);
        }
    }
    if (path.empty()) {
        die("a <path> argument is required");
    }
    const std::string combined = combine_excludes(excludes);
    if (!combined.empty()) {
        opts.exclude = combined;
    }
    return path;
}

static void write_stdout(const std::string &s) {
    std::fwrite(s.data(), 1, s.size(), stdout);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        die("usage: parity-driver {manifest|id|push|fetch|checkout} <args...>");
    }
    const std::string sub = argv[1];
    const std::vector<std::string> rest(argv + 2, argv + argc);

    try {
        if (sub == "manifest") {
            snapdir::ManifestOptions opts;
            const std::string path = parse_path_and_opts(rest, opts);
            snapdir::Manifest m = snapdir::manifest(path, opts);
            // §1.1: emit the raw manifest TEXT byte-exact, incl the trailing \n.
            std::string raw = m.raw;
            if (raw.empty() || raw.back() != '\n') {
                raw += '\n';
            }
            write_stdout(raw);
        } else if (sub == "id") {
            snapdir::ManifestOptions opts;
            const std::string path = parse_path_and_opts(rest, opts);
            write_stdout(snapdir::id(path, opts) + "\n");  // 64-hex + \n
        } else if (sub == "push") {
            if (rest.size() < 2) {
                die("push requires <path> <store_uri>");
            }
            // push <path> <store_uri> [--jobs N]… (tuning args ignored)
            write_stdout(snapdir::push(rest[0], rest[1]).get() + "\n");
        } else if (sub == "fetch") {
            if (rest.size() < 2) {
                die("fetch requires <id> <store_uri>");
            }
            snapdir::fetch(rest[0], rest[1]).get();
        } else if (sub == "checkout") {
            // checkout <id> <store_uri> <dest> → pull(id, store, dest)
            if (rest.size() < 3) {
                die("checkout requires <id> <store_uri> <dest>");
            }
            snapdir::pull(rest[0], rest[1], rest[2]).get();
        } else {
            die("unknown subcommand " + sub);
        }
    } catch (const snapdir::Error &e) {
        die(sub + " failed [" + e.code() + "]: " + e.what());
    }
    return 0;
}
