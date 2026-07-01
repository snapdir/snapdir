// app.cpp — canonical example: snapdir C++ binding CLI
//
// Demonstrates the snapdir C++ header-only RAII wrapper over the C ABI.
// The store URI and credentials are read from the environment:
//   SNAPDIR_S3_STORE_ENDPOINT_URL, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
//
// CLI:
//   app push <dir> <store>              → prints the 64-hex snapshot id
//   app pull <id>  <store> <dest>       → materialises snapshot into dest
//   app id   <dir>                      → prints the 64-hex snapshot id
//   app diff <store@id_a> <store@id_b>  → prints STATUS<TAB>PATH per line
//
// Compile: g++ -std=c++20 app.cpp -I. -L. -lsnapdir_ffi -lpthread -ldl -lm -o app

#include "snapdir.hpp"

#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <stdexcept>
#include <string>
#include <string_view>

namespace fs = std::filesystem;

// Split "store@id" into (store, id). The last '@' is the delimiter.
static std::pair<std::string, std::string> parseRef(const std::string &ref) {
    auto at = ref.rfind('@');
    if (at == std::string::npos)
        return {ref, ""};
    return {ref.substr(0, at), ref.substr(at + 1)};
}

int main(int argc, char *argv[]) {
    if (argc < 2) {
        std::cerr << "usage: app {push|pull|id|diff} [args...]\n";
        return 1;
    }

    std::string_view cmd{argv[1]};

    try {
        if (cmd == "push") {
            // push <dir> <store> — stage dir and upload to store; print snapshot id.
            auto id = snapdir::push(argv[2], argv[3]).get();
            std::cout << id << '\n';

        } else if (cmd == "pull") {
            // pull <id> <store> <dest> — fetch snapshot and materialise into dest.
            snapdir::pull(argv[2], argv[3], fs::path{argv[4]}).get();

        } else if (cmd == "id") {
            // id <dir> — compute and print the snapshot id for dir.
            auto id = snapdir::id(fs::path{argv[2]});
            std::cout << id << '\n';

        } else if (cmd == "diff") {
            // diff <store@id_a> <store@id_b> — compare two pinned snapshots.
            //
            // The binding's diff() compares two STORE contents. To diff two
            // pinned snapshots from the same store we pull each into a temporary
            // directory, push each to its own temporary file store, then diff
            // those two stores.
            auto [storeFrom, idFrom] = parseRef(argv[2]);
            auto [storeTo,   idTo]   = parseRef(argv[3]);

            // Unique temp root under /tmp (container-local, cleaned on exit).
            char tmpl[] = "/tmp/sd-diff-XXXXXX";
            if (!mkdtemp(tmpl)) {
                std::cerr << "mkdtemp failed\n";
                return 1;
            }
            fs::path tmp{tmpl};
            fs::create_directories(tmp / "from");
            fs::create_directories(tmp / "to");

            std::string fstoreFrom = "file://" + (tmp / "store-from").string();
            std::string fstoreTo   = "file://" + (tmp / "store-to").string();

            snapdir::pull(idFrom, storeFrom, tmp / "from").get();
            snapdir::push(tmp / "from", fstoreFrom).get();
            snapdir::pull(idTo, storeTo, tmp / "to").get();
            snapdir::push(tmp / "to", fstoreTo).get();

            auto entries = snapdir::diff(fstoreFrom, fstoreTo).get();

            // Print as STATUS<TAB>PATH per line — matches the snapdir CLI format.
            for (const auto &e : entries) {
                std::cout << static_cast<char>(e.status) << '\t' << e.path << '\n';
            }

            // Cleanup (best effort; container exits anyway).
            fs::remove_all(tmp);

        } else {
            std::cerr << "unknown command: " << cmd << '\n';
            return 1;
        }
    } catch (const snapdir::Error &e) {
        std::cerr << "snapdir error: " << e.what() << '\n';
        return 1;
    } catch (const std::exception &e) {
        std::cerr << "error: " << e.what() << '\n';
        return 1;
    }

    return 0;
}
