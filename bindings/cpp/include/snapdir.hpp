#pragma once

// snapdir.hpp — header-only C++ RAII wrapper over the snapdir-ffi C ABI.
//
// Minimum standard: C++17 (std::optional, std::filesystem, std::future).
// Compile with:  clang++ -std=c++20 -Wall -Wextra -Werror
//                        -fsyntax-only -Iinclude -Ibindings/cpp/include
//
// Every C allocation is freed even when an Error is thrown: StringGuard and
// ErrorGuard are the only two RAII owners of C heap memory.

// The C ABI header is cbindgen plain-C with no `extern "C"` guard of its own;
// wrap it so the declarations get C linkage matching the Rust `#[no_mangle]`
// exports (otherwise C++ name-mangling breaks linking against libsnapdir_ffi).
extern "C" {
#include <snapdir.h>
}

#include <cstdint>
#include <filesystem>
#include <future>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace snapdir {

// ─── Error ───────────────────────────────────────────────────────────────────

/// @brief Exception thrown by every snapdir wrapper function on C-layer failure.
///
/// Inherits from std::runtime_error. The formatted what() string is
/// "[CODE] human-readable message". The stable ABI code is also accessible
/// via code() for programmatic dispatch.
///
/// Stable ABI codes: "IO_ERROR", "HASH_MISMATCH", "STORE_ERROR", "IN_FLUX",
/// "CATALOG_ERROR", "INVALID_ID", "INVALID_STORE", "CONFLICT", "INTERNAL".
class Error : public std::runtime_error {
public:
    /// @brief Construct from an already-inspected (not yet freed) SnapdirError*.
    ///
    /// Copies the code and message strings out of @p e. The caller is responsible
    /// for freeing @p e after construction (or use ErrorGuard which does it for you).
    /// @param e Pointer to the C error object; must not be null.
    explicit Error(const SnapdirError *e)
        : std::runtime_error(make_message(e))
        , code_(snapdir_error_code(e) ? snapdir_error_code(e) : "INTERNAL")
    {}

    /// @brief Return the stable error code string.
    ///
    /// The returned reference is valid for the lifetime of this Error object.
    /// One of the 8 stable ABI codes or "INTERNAL" for unexpected failures.
    /// @return A const reference to the code string (e.g. "IO_ERROR").
    const std::string &code() const noexcept { return code_; }

private:
    std::string code_;

    static std::string make_message(const SnapdirError *e) {
        const char *msg = snapdir_error_message(e);
        const char *code = snapdir_error_code(e);
        std::string result;
        result += '[';
        result += (code ? code : "INTERNAL");
        result += "] ";
        result += (msg ? msg : "(no message)");
        return result;
    }
};

// ─── RAII guards ─────────────────────────────────────────────────────────────

/// @brief RAII owner for a C string returned by the snapdir-ffi ABI.
///
/// Any snapdir_*() function that returns an owned `char*` (manifest text,
/// snapshot ID, JSON diff output, etc.) should be wrapped in a StringGuard
/// immediately after the call. The string is freed with snapdir_string_free()
/// on destruction, even if an exception is in flight.
///
/// Movable, non-copyable. A null pointer is valid and safe to hold.
class StringGuard {
public:
    /// @brief Construct owning @p ptr (may be nullptr).
    /// @param ptr Heap-allocated C string, or nullptr.
    explicit StringGuard(char *ptr = nullptr) noexcept : ptr_(ptr) {}

    /// @brief Destructor — frees the owned string via snapdir_string_free().
    ~StringGuard() noexcept { snapdir_string_free(ptr_); }

    /// @brief Move constructor; transfers ownership from @p other.
    /// @param other Source guard; left holding nullptr after the move.
    StringGuard(StringGuard &&other) noexcept : ptr_(other.ptr_) {
        other.ptr_ = nullptr;
    }
    /// @brief Move assignment; frees the current string, then takes ownership from @p other.
    /// @param other Source guard; left holding nullptr after the move.
    /// @return *this
    StringGuard &operator=(StringGuard &&other) noexcept {
        if (this != &other) {
            snapdir_string_free(ptr_);
            ptr_ = other.ptr_;
            other.ptr_ = nullptr;
        }
        return *this;
    }

    StringGuard(const StringGuard &) = delete;
    StringGuard &operator=(const StringGuard &) = delete;

    /// @brief Return the raw C string pointer (may be nullptr).
    /// @return Pointer to the null-terminated string, or nullptr.
    const char *c_str() const noexcept { return ptr_; }

    /// @brief Convert the owned string to std::string.
    ///
    /// Returns an empty string if the internal pointer is nullptr.
    /// @return A copy of the string as std::string.
    std::string str() const { return ptr_ ? std::string(ptr_) : std::string(); }

    /// @brief Return true if the owned pointer is non-null.
    explicit operator bool() const noexcept { return ptr_ != nullptr; }

    /// @brief Release ownership without freeing; caller takes responsibility for freeing.
    ///
    /// After this call the guard holds nullptr. Use when handing the pointer
    /// back to a C API that takes ownership.
    /// @return The previously owned pointer (caller must call snapdir_string_free()).
    char *release() noexcept {
        char *p = ptr_;
        ptr_ = nullptr;
        return p;
    }

private:
    char *ptr_;
};

/// @brief RAII owner for a SnapdirError* written by snapdir-ffi out-parameters.
///
/// C functions in the snapdir-ffi ABI signal errors by writing a non-null
/// SnapdirError* into an `err_out` parameter. Wrap that parameter slot with
/// out_param() and the guard will free the error object on destruction.
/// Call throw_if_set() to convert any error into a snapdir::Error exception.
///
/// Movable, non-copyable.
class ErrorGuard {
public:
    /// @brief Construct with an optional pre-existing error pointer.
    /// @param ptr Existing SnapdirError* or nullptr (default).
    explicit ErrorGuard(SnapdirError *ptr = nullptr) noexcept : ptr_(ptr) {}

    /// @brief Destructor — frees the owned error via snapdir_error_free().
    ~ErrorGuard() noexcept { snapdir_error_free(ptr_); }

    /// @brief Move constructor; transfers ownership from @p other.
    /// @param other Source guard; left holding nullptr after the move.
    ErrorGuard(ErrorGuard &&other) noexcept : ptr_(other.ptr_) {
        other.ptr_ = nullptr;
    }
    /// @brief Move assignment; frees the current error, then takes ownership from @p other.
    /// @param other Source guard; left holding nullptr after the move.
    /// @return *this
    ErrorGuard &operator=(ErrorGuard &&other) noexcept {
        if (this != &other) {
            snapdir_error_free(ptr_);
            ptr_ = other.ptr_;
            other.ptr_ = nullptr;
        }
        return *this;
    }

    ErrorGuard(const ErrorGuard &) = delete;
    ErrorGuard &operator=(const ErrorGuard &) = delete;

    /// @brief Return true if an error was set (internal pointer is non-null).
    explicit operator bool() const noexcept { return ptr_ != nullptr; }

    /// @brief Throw snapdir::Error if an error is set; no-op otherwise.
    ///
    /// The guard frees the underlying SnapdirError* after the Error object
    /// is fully constructed so there is no double-free if the throw is caught.
    /// @throws snapdir::Error if the guard holds a non-null error pointer.
    void throw_if_set() {
        if (ptr_) {
            Error ex(ptr_);         // copy code + message strings
            snapdir_error_free(ptr_);
            ptr_ = nullptr;
            throw ex;
        }
    }

    /// @brief Return a pointer to the internal pointer for use as an err_out argument.
    ///
    /// Pass &guard.out_param() — actually, pass guard.out_param() — to any
    /// snapdir_*() function's err_out parameter. The C function writes a
    /// SnapdirError* here on failure; the guard frees it.
    /// @return Pointer to the internal SnapdirError* slot.
    SnapdirError **out_param() noexcept { return &ptr_; }

private:
    SnapdirError *ptr_;
};

// ─── Result types ────────────────────────────────────────────────────────────

/// @brief Identifies the filesystem entry type of a manifest entry.
///
/// Used in ManifestEntry::type to distinguish files, directories, and symlinks.
/// Note: under --no-follow, relative or dangling symlinks are OMITTED from the
/// manifest entirely; the 'L' variant only appears for resolved symlinks when
/// symlink-following is active.
enum class PathType : char {
    File      = 'F', ///< Regular file.
    Directory = 'D', ///< Directory.
    Symlink   = 'L', ///< Symbolic link (only present when following symlinks).
};

/// @brief Change indicator for an entry returned by diff().
///
/// Corresponds to the single-character status field in the snapdir JSON diff
/// output: 'A' for added, 'D' for deleted, 'M' for modified, '=' for unchanged.
enum class DiffStatus : char {
    Added     = 'A', ///< Entry exists in to_uri but not from_uri.
    Deleted   = 'D', ///< Entry exists in from_uri but not to_uri.
    Modified  = 'M', ///< Entry exists in both stores but content differs.
    Unchanged = '=', ///< Entry is identical in both stores.
};

/// @brief One parsed line from a snapshot manifest.
///
/// A snapshot manifest is the text representation of a directory tree produced
/// by manifest(). Each line (excluding comment lines starting with '#') describes
/// one filesystem entry.
struct ManifestEntry {
    PathType                type;       ///< Entry kind: File, Directory, or Symlink.
    std::uint32_t           perm;       ///< POSIX mode bits in octal (e.g. 0644).
    std::string             checksum;   ///< 64-char lowercase BLAKE3 hex digest; empty for directories.
    std::uint64_t           size;       ///< File size in bytes (0 for directories).
    std::filesystem::path   path;       ///< Relative (or absolute when ManifestOptions::absolute is set) path.
};

/// @brief A parsed snapshot manifest returned by manifest().
///
/// Holds both the raw manifest text (as returned by the C library) and the
/// parsed entries for convenient iteration. The raw text can be passed to
/// id_from_manifest() without re-walking the directory.
struct Manifest {
    std::string                  raw;     ///< Full manifest text exactly as returned by the C library.
    std::vector<ManifestEntry>   entries; ///< Parsed entries derived from raw; excludes comment/blank lines.
};

/// @brief One entry from the result of a diff() operation.
///
/// Describes a single path that differs (or is unchanged, when
/// DiffOptions::include_unchanged is true) between two store URIs.
struct DiffEntry {
    DiffStatus              status; ///< Change indicator (Added, Deleted, Modified, or Unchanged).
    std::filesystem::path   path;   ///< Path of the affected entry relative to the snapshot root.
};

// ─── ManifestOptions ─────────────────────────────────────────────────────────

/// @brief Options controlling the directory-walk behaviour for manifest() and id().
///
/// All fields default to the C ABI NULL/0/false equivalents so a default-constructed
/// ManifestOptions is equivalent to calling the C function with all optional parameters
/// set to nullptr/0/false.
struct ManifestOptions {
    std::optional<std::string>  exclude;        ///< Extended-regex exclusion pattern; paths matching this are skipped.
    std::uint32_t               walk_jobs = 0;  ///< Parallel walk threads; 0 = auto (CPU count).
    bool                        absolute  = false; ///< Emit absolute paths in the manifest instead of relative ones.
    bool                        no_follow = false; ///< Do not follow symlinks; relative/dangling symlinks are omitted.
    std::optional<std::string>  checksum_bin;   ///< Path or name of the checksum binary; nullptr uses the "b3sum" default.
    std::optional<std::string>  cache_dir;      ///< Override the cache directory; nullptr uses the library default.
    std::optional<std::string>  catalog;        ///< Catalog adapter name or URI; nullptr uses the default adapter.
};

// ─── Internal helpers ─────────────────────────────────────────────────────────

namespace detail {

// init() calls snapdir_init() once. The underlying fn is idempotent, so
// calling it unconditionally before each public fn is safe and cheap.
inline void init() noexcept { snapdir_init(); }

// optional_cstr returns the c_str() of an optional<string>, or nullptr.
inline const char *optional_cstr(const std::optional<std::string> &opt) noexcept {
    return opt.has_value() ? opt->c_str() : nullptr;
}

// parse_manifest_text splits the manifest TEXT into ManifestEntry records.
// Format per line: TYPE PERM CHECKSUM SIZE PATH
// Lines starting with '#' or blank are skipped.
inline std::vector<ManifestEntry> parse_manifest_text(const std::string &text) {
    std::vector<ManifestEntry> entries;
    std::istringstream ss(text);
    std::string line;
    while (std::getline(ss, line)) {
        // Strip trailing CR for CRLF inputs.
        if (!line.empty() && line.back() == '\r') {
            line.pop_back();
        }
        if (line.empty() || line[0] == '#') {
            continue;
        }
        std::istringstream ls(line);
        std::string type_str, perm_str, checksum_str, size_str, path_str;
        if (!(ls >> type_str >> perm_str >> checksum_str >> size_str)) {
            continue;
        }
        // The path may contain spaces; read the remainder.
        if (!std::getline(ls, path_str)) {
            continue;
        }
        // Skip leading space left by >> extraction.
        if (!path_str.empty() && path_str[0] == ' ') {
            path_str.erase(0, 1);
        }
        if (type_str.empty() || path_str.empty()) {
            continue;
        }

        PathType pt;
        switch (type_str[0]) {
            case 'F': pt = PathType::File;      break;
            case 'D': pt = PathType::Directory; break;
            case 'L': pt = PathType::Symlink;   break;
            default:  continue;
        }

        std::uint32_t perm = 0;
        try {
            perm = static_cast<std::uint32_t>(std::stoul(perm_str, nullptr, 8));
        } catch (...) { continue; }

        std::uint64_t size = 0;
        try {
            size = std::stoull(size_str);
        } catch (...) { continue; }

        entries.push_back(ManifestEntry{
            pt,
            perm,
            checksum_str,
            size,
            std::filesystem::path(path_str),
        });
    }
    return entries;
}

// unescape_json_string decodes a JSON string value (the content between the
// outer double-quotes, not including the quotes themselves). Handles the common
// escape sequences: \\ \" \/ \n \r \t \b \f. Unknown escapes are passed
// through as-is. The snapdir diff JSON paths are plain ASCII (./relative) so
// full Unicode handling is not required, but we handle the sequences above so
// paths like ".\/sub" round-trip correctly.
inline std::string unescape_json_string(const std::string &s) {
    std::string out;
    out.reserve(s.size());
    for (std::size_t i = 0; i < s.size(); ++i) {
        if (s[i] == '\\' && i + 1 < s.size()) {
            ++i;
            switch (s[i]) {
                case '"':  out += '"';  break;
                case '\\': out += '\\'; break;
                case '/':  out += '/';  break;
                case 'n':  out += '\n'; break;
                case 'r':  out += '\r'; break;
                case 't':  out += '\t'; break;
                case 'b':  out += '\b'; break;
                case 'f':  out += '\f'; break;
                default:   out += '\\'; out += s[i]; break;
            }
        } else {
            out += s[i];
        }
    }
    return out;
}

// scan_json_string_end finds the index of the closing '"' of a JSON string
// that starts at open_quote (the position of the opening '"'). Returns
// std::string::npos if unterminated. Correctly skips escaped characters so
// a '\"' inside the value does not end the scan prematurely.
inline std::size_t scan_json_string_end(const std::string &s,
                                        std::size_t open_quote) {
    // open_quote points at the opening '"'; the value starts at open_quote+1.
    for (std::size_t i = open_quote + 1; i < s.size(); ++i) {
        if (s[i] == '\\') {
            ++i; // skip escaped character
        } else if (s[i] == '"') {
            return i; // closing quote
        }
    }
    return std::string::npos;
}

// parse_diff_json does a minimal hand-rolled parse of the diff JSON array.
// Shape: [{"status":"A","path":"./x"}, ...]
// Status is one of "A","D","M","=". Objects are parsed independently so the
// status and path values always come from the same object even if the field
// order is not fixed. JSON string escapes in path values are decoded.
inline std::vector<DiffEntry> parse_diff_json(const std::string &json) {
    std::vector<DiffEntry> entries;
    std::size_t pos = 0;
    while (pos < json.size()) {
        // Locate the opening brace of the next JSON object.
        auto obj_open = json.find('{', pos);
        if (obj_open == std::string::npos) break;

        // Find the matching closing brace. We track nesting depth and skip
        // over JSON strings (including escaped characters) to avoid
        // misidentifying a '}' inside a string value.
        int depth = 0;
        std::size_t obj_close = std::string::npos;
        for (std::size_t i = obj_open; i < json.size(); ++i) {
            if (json[i] == '\\') {
                ++i; // skip escaped character inside a string
            } else if (json[i] == '"') {
                // Skip the string body so braces inside values are ignored.
                i = scan_json_string_end(json, i);
                if (i == std::string::npos) break;
            } else if (json[i] == '{') {
                ++depth;
            } else if (json[i] == '}') {
                --depth;
                if (depth == 0) {
                    obj_close = i;
                    break;
                }
            }
        }
        if (obj_close == std::string::npos) break;

        // Work only within [obj_open, obj_close].
        const std::string obj = json.substr(obj_open, obj_close - obj_open + 1);

        // Extract "status":"X" — value is exactly one character.
        char status_char = '\0';
        {
            auto st = obj.find("\"status\":\"");
            if (st != std::string::npos) {
                st += 10; // skip past "status":"
                if (st < obj.size()) status_char = obj[st];
            }
        }

        // Extract "path":"Y" with JSON escape decoding.
        std::string path_str;
        {
            auto pa = obj.find("\"path\":\"");
            if (pa != std::string::npos) {
                pa += 7; // now points at the opening '"' of the value
                auto pa_end = scan_json_string_end(obj, pa);
                if (pa_end != std::string::npos) {
                    path_str = unescape_json_string(
                        obj.substr(pa + 1, pa_end - pa - 1));
                }
            }
        }

        DiffStatus ds;
        switch (status_char) {
            case 'A': ds = DiffStatus::Added;     break;
            case 'D': ds = DiffStatus::Deleted;   break;
            case 'M': ds = DiffStatus::Modified;  break;
            case '=': ds = DiffStatus::Unchanged; break;
            default:  pos = obj_close + 1; continue;
        }

        if (!path_str.empty()) {
            entries.push_back(DiffEntry{ds, std::filesystem::path(path_str)});
        }
        pos = obj_close + 1;
    }
    return entries;
}

} // namespace detail

// ─── Free functions (synchronous) ────────────────────────────────────────────

/// @brief Return the snapdir-api library version string (e.g. "1.10.0").
///
/// The underlying C string has static lifetime; this copies it into a
/// std::string for safe use across thread and object boundaries.
/// @return The library version as a std::string; never empty in a correctly
///         linked binary.
inline std::string version() {
    detail::init();
    const char *v = snapdir_version();
    return v ? std::string(v) : std::string();
}

/// @brief Walk @p path and return a parsed directory manifest.
///
/// Hashes every file under @p path using BLAKE3 and returns both the raw
/// manifest text and the parsed entries. The walk is parallelised according
/// to opts.walk_jobs (0 = auto). Symlinks are followed by default; set
/// opts.no_follow to omit relative/dangling symlinks instead.
///
/// @param path  Directory to walk (must exist and be accessible).
/// @param opts  Walk options (exclusion pattern, parallelism, symlink handling, etc.).
/// @return      A Manifest containing the raw text and all parsed entries.
/// @throws snapdir::Error on I/O failure, hash mismatch, or catalog error.
inline Manifest manifest(const std::filesystem::path &path,
                         const ManifestOptions &opts = {})
{
    detail::init();
    ErrorGuard  eg;
    StringGuard sg(snapdir_manifest(
        path.c_str(),
        detail::optional_cstr(opts.exclude),
        opts.walk_jobs,
        opts.absolute,
        opts.no_follow,
        detail::optional_cstr(opts.checksum_bin),
        detail::optional_cstr(opts.cache_dir),
        detail::optional_cstr(opts.catalog),
        eg.out_param()
    ));
    eg.throw_if_set();
    std::string raw = sg.str();
    return Manifest{raw, detail::parse_manifest_text(raw)};
}

/// @brief Compute the snapshot id from a Manifest previously returned by manifest().
///
/// This is a pure synchronous operation with no filesystem I/O: it hashes the
/// raw manifest text to produce the 64-char BLAKE3 snapshot id. Use this to
/// avoid re-walking the directory when the Manifest is already available.
///
/// @param m  A Manifest returned by a prior call to manifest().
/// @return   64-char lowercase hex BLAKE3 snapshot id string.
/// @throws snapdir::Error if the C library signals an error (e.g. empty manifest).
inline std::string id_from_manifest(const Manifest &m) {
    detail::init();
    ErrorGuard  eg;
    StringGuard sg(snapdir_id_from_manifest_text(m.raw.c_str(), eg.out_param()));
    eg.throw_if_set();
    return sg.str();
}

/// @brief Compute the snapshot id for the directory at @p path.
///
/// Returns the 64-char lowercase hex BLAKE3 snapshot id that uniquely identifies
/// the content of the directory tree at @p path.
///
/// When opts.no_follow or opts.absolute are set the function routes through
/// manifest() → id_from_manifest() because snapdir_id() does not expose those
/// parameters. For the common case (no_follow=false, absolute=false) the fast
/// direct path via snapdir_id() is used without materialising a Manifest object.
///
/// @param path  Directory to snapshot (must exist and be accessible).
/// @param opts  Walk options; see ManifestOptions.
/// @return      64-char lowercase hex BLAKE3 snapshot id.
/// @throws snapdir::Error on I/O failure, hash mismatch, or catalog error.
inline std::string id(const std::filesystem::path &path,
                      const ManifestOptions &opts = {})
{
    if (opts.no_follow || opts.absolute) {
        return id_from_manifest(manifest(path, opts));
    }
    detail::init();
    ErrorGuard  eg;
    StringGuard sg(snapdir_id(
        path.c_str(),
        detail::optional_cstr(opts.exclude),
        opts.walk_jobs,
        detail::optional_cstr(opts.cache_dir),
        eg.out_param()
    ));
    eg.throw_if_set();
    return sg.str();
}

// ─── Async operations ────────────────────────────────────────────────────────
//
// Every blocking C function runs inside std::async so the caller can wait on
// the returned std::future<T>, propagate cancellation, or compose with other
// futures. The async task owns all C strings for its lifetime.

/// @brief Options controlling the push() operation.
///
/// All fields are optional; defaults reproduce the C ABI behaviour (auto job
/// count, no rate limit, default retry count, default cache directory).
struct PushOptions {
    std::optional<std::string>  source_id;       ///< If set, push a pre-staged snapshot by id instead of walking path.
    std::uint32_t               jobs       = 0;  ///< Upload parallelism; 0 = library default.
    std::optional<std::string>  limit_rate;      ///< Bandwidth cap (e.g. "10M" for 10 MiB/s); nullptr = unlimited.
    std::uint32_t               max_retries = 0; ///< Maximum retry attempts on transient errors; 0 = library default (5).
    std::optional<std::string>  cache_dir;       ///< Override the local cache directory; nullptr = library default.
};

/// @brief Stage and push the directory at @p path to @p store_uri.
///
/// Walks @p path, builds a content-addressed snapshot, and uploads the snapshot
/// objects to the remote store. The operation runs asynchronously in a background
/// thread; call .get() on the returned future to block until completion.
///
/// @param path       Local directory to snapshot and push.
/// @param store_uri  Destination store URI (e.g. "file:///mnt/store" or "gcs://bucket/prefix").
/// @param opts       Push options (parallelism, rate limit, retry count, etc.).
/// @return           A future resolving to the 64-char snapshot id string.
/// @throws snapdir::Error (from the future) on I/O, store, or hash error.
inline std::future<std::string> push(const std::filesystem::path &path,
                                     const std::string &store_uri,
                                     const PushOptions &opts = {})
{
    // Copy all strings so the async task is self-contained.
    std::string path_s      = path.string();
    std::string store_s     = store_uri;
    PushOptions opts_copy   = opts;

    return std::async(std::launch::async, [path_s, store_s, opts_copy]() -> std::string {
        detail::init();
        ErrorGuard eg;
        const char *source_id_c  = detail::optional_cstr(opts_copy.source_id);
        const char *limit_rate_c = detail::optional_cstr(opts_copy.limit_rate);
        const char *cache_dir_c  = detail::optional_cstr(opts_copy.cache_dir);
        StringGuard sg(snapdir_push_blocking(
            path_s.c_str(),
            source_id_c,
            store_s.c_str(),
            opts_copy.jobs,
            limit_rate_c,
            opts_copy.max_retries,
            cache_dir_c,
            eg.out_param()
        ));
        eg.throw_if_set();
        return sg.str();
    });
}

/// @brief Options controlling the fetch() operation.
struct FetchOptions {
    std::uint32_t jobs = 0; ///< Download parallelism; 0 = library default.
};

/// @brief Download a snapshot from @p store_uri into the local cache.
///
/// Fetches all objects for @p snapshot_id from the remote store into the local
/// content-addressed cache without materialising the files on disk. Call pull()
/// instead if you want the files written to a destination directory.
///
/// The operation runs asynchronously; call .get() on the future to block.
///
/// @param snapshot_id  64-char lowercase hex BLAKE3 snapshot id to fetch.
/// @param store_uri    Source store URI.
/// @param opts         Fetch options (download parallelism).
/// @return             A future that resolves to void on success.
/// @throws snapdir::Error (from the future) on store, network, or hash error.
inline std::future<void> fetch(const std::string &snapshot_id,
                               const std::string &store_uri,
                               const FetchOptions &opts = {})
{
    std::string id_s    = snapshot_id;
    std::string store_s = store_uri;
    FetchOptions opts_copy = opts;

    return std::async(std::launch::async, [id_s, store_s, opts_copy]() {
        detail::init();
        ErrorGuard eg;
        int rc = snapdir_fetch_blocking(
            id_s.c_str(),
            store_s.c_str(),
            opts_copy.jobs,
            eg.out_param()
        );
        if (rc != 0) {
            eg.throw_if_set();
            // rc != 0 but no error set — shouldn't happen, but be defensive.
            throw Error(nullptr);  // NOLINT: only reached on ABI contract violation
        }
    });
}

/// @brief Options controlling the pull() operation.
struct PullOptions {
    bool          delete_extra = false; ///< Delete files in dest_path that are not in the snapshot (mirror mode).
    std::uint32_t jobs        = 0;      ///< Materialisation parallelism; 0 = library default.
};

/// @brief Fetch a snapshot and materialise it into @p dest_path.
///
/// Combines fetch() and materialisation: downloads snapshot objects for
/// @p snapshot_id from @p store_uri and writes the files into @p dest_path.
/// When opts.delete_extra is true, files in @p dest_path that are absent from
/// the snapshot are deleted, making the directory an exact mirror of the snapshot.
///
/// The operation runs asynchronously; call .get() on the future to block.
///
/// @param snapshot_id  64-char lowercase hex BLAKE3 snapshot id to pull.
/// @param store_uri    Source store URI.
/// @param dest_path    Local directory to materialise the snapshot into.
/// @param opts         Pull options (delete_extra, parallelism).
/// @return             A future that resolves to void on success.
/// @throws snapdir::Error (from the future) on store, I/O, or hash error.
inline std::future<void> pull(const std::string &snapshot_id,
                              const std::string &store_uri,
                              const std::filesystem::path &dest_path,
                              const PullOptions &opts = {})
{
    std::string id_s    = snapshot_id;
    std::string store_s = store_uri;
    std::string dest_s  = dest_path.string();
    PullOptions opts_copy = opts;

    return std::async(std::launch::async, [id_s, store_s, dest_s, opts_copy]() {
        detail::init();
        ErrorGuard eg;
        int rc = snapdir_pull_blocking(
            id_s.c_str(),
            store_s.c_str(),
            dest_s.c_str(),
            opts_copy.delete_extra,
            opts_copy.jobs,
            eg.out_param()
        );
        if (rc != 0) {
            eg.throw_if_set();
            throw Error(nullptr);
        }
    });
}

/// @brief Options controlling the diff() operation.
struct DiffOptions {
    std::optional<std::string>  snapshot_id;            ///< If set, restrict the diff to this snapshot id; nullptr = A→B cross-store diff.
    bool                        include_unchanged = false; ///< Include entries with DiffStatus::Unchanged in the result.
    std::optional<std::string>  on_conflict;            ///< Conflict resolution strategy ("error", "ours", "theirs"); nullptr = "error".
};

/// @brief Compute the difference between two store URIs.
///
/// Compares the content of @p from_uri and @p to_uri and returns a list of
/// DiffEntry records describing which paths were added, deleted, modified, or
/// (optionally) unchanged. JSON string escapes in path values are decoded.
///
/// The operation runs asynchronously in a background thread.
///
/// @param from_uri  Source store URI (baseline).
/// @param to_uri    Target store URI (to compare against).
/// @param opts      Diff options (snapshot filter, include_unchanged, conflict resolution).
/// @return          A future resolving to a vector of DiffEntry records.
/// @throws snapdir::Error (from the future) on store, I/O, or conflict error.
inline std::future<std::vector<DiffEntry>> diff(const std::string &from_uri,
                                                const std::string &to_uri,
                                                const DiffOptions &opts = {})
{
    std::string from_s = from_uri;
    std::string to_s   = to_uri;
    DiffOptions opts_copy = opts;

    return std::async(std::launch::async, [from_s, to_s, opts_copy]() -> std::vector<DiffEntry> {
        detail::init();

        // Build NULL-terminated C arrays for from_uris and to_uris.
        const char *from_arr[2] = {from_s.c_str(), nullptr};
        const char *to_arr[2]   = {to_s.c_str(),   nullptr};

        ErrorGuard  eg;
        const char *snap_id_c      = detail::optional_cstr(opts_copy.snapshot_id);
        const char *on_conflict_c  = detail::optional_cstr(opts_copy.on_conflict);

        StringGuard sg(snapdir_diff_json(
            from_arr,
            to_arr,
            snap_id_c,
            opts_copy.include_unchanged,
            on_conflict_c,
            eg.out_param()
        ));
        eg.throw_if_set();
        return detail::parse_diff_json(sg.str());
    });
}

} // namespace snapdir
