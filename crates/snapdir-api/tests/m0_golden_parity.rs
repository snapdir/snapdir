//! M0 — GOLDEN BIT-IDENTICAL PARITY of `snapdir-api` vs the released 1.10.0 oracle.
//!
//! AUTHORED FROM THE SPEC ONLY (`.gatesmith/reviews/m0-public-api.md` §6/§8 + the
//! frozen manifest contract anchored by `crates/snapdir-core/tests/compat_golden.rs`).
//! The `snapdir-api` `src/` wiring was NOT read while authoring this suite.
//!
//! THE ORACLE
//! ==========
//! The workspace is version 1.10.0 and `snapdir-api` is purely additive, so the
//! workspace **`snapdir` CLI binary IS the 1.10.0 oracle**. We invoke it exactly the
//! way the existing CLI integration tests do — `assert_cmd::cargo::cargo_bin("snapdir")`
//! (see `crates/snapdir-cli/tests/manifest.rs` + `diff.rs`). The bin target lives in the
//! `snapdir` crate, so `CARGO_BIN_EXE_snapdir` is not set for this crate's tests;
//! `assert_cmd`'s lookup falls back to the shared target dir. Under
//! `cargo test --workspace` the binary is always built first.
//!
//! >>> IMPL-GATE REQUIREMENT (the lane owner MUST satisfy this to make the suite
//! >>> runnable): add `assert_cmd` as a `[dev-dependencies]` of `crates/snapdir-api`.
//! >>> Until then this file is staged un-runnable in `.gatesmith/pending-tests/` and
//! >>> the workspace keeps compiling. This is EXPECTED for an authoring gate.
//!
//! THE CONTRACT THIS PINS (§8 "golden bit-identical parity vs the pinned 1.10.0 oracle"):
//! for a battery of fixture trees, `snapdir_api::manifest(...).raw` is **byte-for-byte
//! identical** to `snapdir manifest <fixture>` stdout (every byte: line order, the
//! `D ./` root line, octal perms, sizes, `./` vs `--absolute`, trailing newline);
//! `snapdir_api::id(...).to_hex()` is the **identical** 64-char lowercase hex as
//! `snapdir id <fixture>`; `id_from_manifest(&manifest(..)) == id(..)`; and
//! `snapdir_api::diff(..)` classifies each path identically to `snapdir diff --json`.
//!
//! Fixtures cover: empty dir, single file, deep nesting (5+ levels), unicode/space/
//! symbol filenames, symlinks (to file + dir), identical-content dedup, and mixed
//! permissions (0644/0600/0755).

// `clippy::pedantic` is enabled workspace-wide; suppress test-only stylistic lints
// (mirroring sibling suites) so the staged file compiles under `-D warnings` WITHOUT
// touching any assertion.
#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown,
    clippy::missing_panics_doc,
    clippy::unreadable_literal,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::unnecessary_to_owned,
    clippy::trivially_copy_pass_by_ref
)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use snapdir_api::{ChecksumBin, ManifestOptions, StoreUri};

// ---------------------------------------------------------------------------
// Oracle plumbing — drive the workspace `snapdir` binary (the 1.10.0 oracle).
// ---------------------------------------------------------------------------

/// Path to the compiled `snapdir` binary under test (the 1.10.0 oracle).
fn snapdir_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("snapdir")
}

/// A unique temp directory; created and returned.
fn temp_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "snapdir-api-golden-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Runs `snapdir <args>` with `extra_env`, asserts exit 0, returns stdout VERBATIM
/// (no trimming — the trailing newline is part of the byte contract for `manifest`).
fn oracle_stdout(args: &[&str], extra_env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(snapdir_bin());
    cmd.args(args)
        .env_remove("SNAPDIR_STORE")
        .env_remove("SNAPDIR_OBJECTS_STORE")
        .env_remove("SNAPDIR_MANIFEST_CONTEXT");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run snapdir oracle");
    assert!(
        out.status.success(),
        "oracle `snapdir {args:?}` exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("oracle stdout is UTF-8")
}

/// Computes the snapshot id for a manifest text by piping it into `snapdir id`
/// (which reads a manifest from stdin when stdin is not a TTY). This is the
/// canonical oracle path for any manifest option combination because `snapdir id`
/// does not accept `--no-follow` or `--absolute` as direct flags.
fn oracle_id_from_manifest_text(manifest_text: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(snapdir_bin())
        .arg("id")
        .env_remove("SNAPDIR_STORE")
        .env_remove("SNAPDIR_OBJECTS_STORE")
        .env_remove("SNAPDIR_MANIFEST_CONTEXT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn snapdir id");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(manifest_text.as_bytes())
        .expect("write manifest to stdin");
    let out = child.wait_with_output().expect("wait for snapdir id");
    assert!(
        out.status.success(),
        "oracle `snapdir id` (stdin) exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("oracle id stdout is UTF-8")
}

/// Sets a path's permission bits.
fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

// ---------------------------------------------------------------------------
// PARITY HELPERS — the core byte-exact assertions used by every fixture test.
// ---------------------------------------------------------------------------

/// Renders the API manifest's frozen text. Per §3, `Manifest.raw` IS "the core
/// manifest's rendered text (Display)" — i.e. exactly what the CLI emits MINUS the
/// CLI's trailing newline (the CLI `manifest` subcommand appends a `\n`). We
/// normalize both sides to a single trailing `\n` so the comparison pins the line
/// CONTENT + ORDER + trailing-newline contract without being defeated by whether
/// `.raw` happens to carry the final newline. The byte-exactness of EVERY line is
/// still asserted.
fn api_raw_normalized(raw: &str) -> String {
    let trimmed = raw.strip_suffix('\n').unwrap_or(raw);
    format!("{trimmed}\n")
}

/// Asserts byte-for-byte manifest parity between the API and the oracle binary for
/// `fixture` under `opts`/`cli_args`. `cli_args` are the extra flags appended after
/// `manifest` (e.g. `["--absolute"]`); they MUST express the same options as `opts`.
fn assert_manifest_parity(label: &str, fixture: &Path, opts: &ManifestOptions, cli_args: &[&str]) {
    let fixture_str = fixture.to_string_lossy().into_owned();
    let mut args = vec!["manifest"];
    args.extend_from_slice(cli_args);
    args.push(&fixture_str);
    let oracle = oracle_stdout(&args, &[]);

    let api = snapdir_api::manifest(fixture, opts)
        .unwrap_or_else(|e| panic!("[{label}] snapdir_api::manifest failed: {e}"));

    // BYTE-IDENTICAL: every line, ordering, the `D ./` root line, octal perms, size,
    // `./` vs absolute path rendering, trailing newline.
    assert_eq!(
        api_raw_normalized(&api.raw),
        oracle,
        "[{label}] manifest text must be BYTE-IDENTICAL to the 1.10.0 oracle.\n\
         --- api .raw (normalized) ---\n{}\n--- oracle stdout ---\n{}",
        api_raw_normalized(&api.raw),
        oracle,
    );

    // Spot-pin the frozen contract markers that bindings will be measured against:
    // the very FIRST line is the `D <octal> <hex> <size> ./` root (or absolute root).
    let first = oracle.lines().next().expect("manifest has a root line");
    assert!(
        first.starts_with("D "),
        "[{label}] root line must be a directory `D` line, got {first:?}"
    );
    if cli_args.contains(&"--absolute") {
        assert!(
            first.ends_with(&format!("{fixture_str}/")) || first.ends_with(&fixture_str),
            "[{label}] --absolute root path must be the fixture dir, got {first:?}"
        );
    } else {
        assert!(
            first.ends_with(" ./"),
            "[{label}] relative root path must be `./`, got {first:?}"
        );
    }
}

/// Asserts snapshot-id parity (API hex == oracle hex == 64 lowercase hex) AND the
/// `id_from_manifest(manifest(..)) == id(..)` self-consistency invariant.
///
/// The oracle for the id is `snapdir manifest [opts] <fixture> | snapdir id` (piped) because
/// `snapdir id` does NOT accept `--no-follow` or `--absolute` as flags — those are only on
/// `snapdir manifest`. Piping the manifest through `snapdir id` produces the canonical id for
/// any combination of manifest options, exactly mirroring `id_from_manifest(manifest(..))`.
fn assert_id_parity(label: &str, fixture: &Path, opts: &ManifestOptions, cli_args: &[&str]) {
    let fixture_str = fixture.to_string_lossy().into_owned();

    // Oracle: `snapdir manifest [opts] <fixture> | snapdir id`.
    // `snapdir id` only supports --exclude (via WalkArgs); it does NOT accept
    // --absolute or --no-follow. Piping the manifest through `snapdir id` is the
    // canonical way to obtain the id for any manifest option combination.
    let mut manifest_args = vec!["manifest"];
    manifest_args.extend_from_slice(cli_args);
    manifest_args.push(&fixture_str);
    let manifest_text = oracle_stdout(&manifest_args, &[]);

    // Pipe the manifest text into `snapdir id` (reads manifest from stdin when not a TTY).
    let oracle_id_raw = oracle_id_from_manifest_text(&manifest_text);
    let oracle_id = oracle_id_raw.trim_end();
    assert_eq!(oracle_id.len(), 64, "[{label}] oracle id is 64 hex chars");
    assert!(
        oracle_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "[{label}] oracle id must be lowercase hex, got {oracle_id:?}"
    );

    let api_id = snapdir_api::id(fixture, opts)
        .unwrap_or_else(|e| panic!("[{label}] snapdir_api::id failed: {e}"));
    let api_hex = api_id.to_hex();

    assert_eq!(
        api_hex, oracle_id,
        "[{label}] snapshot id hex must be IDENTICAL to the 1.10.0 oracle"
    );
    assert_eq!(api_hex.len(), 64, "[{label}] api id is 64 hex chars");
    assert_eq!(
        api_hex,
        api_hex.to_lowercase(),
        "[{label}] api id must be lowercase"
    );

    // id_from_manifest(manifest(..)) == id(..): the pure path reproduces the walk path.
    let m = snapdir_api::manifest(fixture, opts)
        .unwrap_or_else(|e| panic!("[{label}] snapdir_api::manifest failed: {e}"));
    let from_manifest = snapdir_api::id_from_manifest(&m);
    assert_eq!(
        from_manifest.to_hex(),
        api_hex,
        "[{label}] id_from_manifest(manifest(..)) must equal id(..)"
    );
    assert_eq!(
        from_manifest, api_id,
        "[{label}] id_from_manifest must equal id as a SnapshotId value"
    );
}

/// Runs BOTH the manifest-parity and id-parity assertions for the default option set.
fn assert_full_parity_default(label: &str, fixture: &Path) {
    let opts = ManifestOptions::default();
    assert_manifest_parity(label, fixture, &opts, &[]);
    assert_id_parity(label, fixture, &opts, &[]);
}

// ===========================================================================
// FIXTURE PARITY TESTS — each pins one tree shape against the 1.10.0 oracle.
// ===========================================================================

#[test]
fn parity_empty_dir() {
    // FIXTURE: an empty directory — the manifest is the lone `D ./` root line; the id
    // is BLAKE3 of that one-line text. Degenerate-input parity.
    let root = temp_dir("empty");
    chmod(&root, 0o755);
    assert_full_parity_default("empty_dir", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_single_file() {
    // FIXTURE: one file. Pins the F-line (type/perm/checksum/size/path) + the root
    // D-line merkle over a single child, byte-identical to the oracle.
    let root = temp_dir("single");
    fs::write(root.join("a.txt"), b"hello").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root, 0o755);
    assert_full_parity_default("single_file", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_nested_five_levels() {
    // FIXTURE: 5+ levels deep — pins the `sort -k5` path ordering of nested D/F lines
    // and the recursive merkle across many entries.
    let root = temp_dir("nested");
    let deep = root.join("a/bb/ccc/dddd/eeeee");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("leaf.txt"), b"deep-leaf-content").unwrap();
    fs::write(root.join("a/top.txt"), b"top").unwrap();
    fs::write(root.join("a/bb/mid.txt"), b"middle").unwrap();
    chmod(&deep.join("leaf.txt"), 0o644);
    chmod(&root.join("a/top.txt"), 0o644);
    chmod(&root.join("a/bb/mid.txt"), 0o644);
    // dirs 0755 (deterministic perms => deterministic D lines)
    for d in [
        "a",
        "a/bb",
        "a/bb/ccc",
        "a/bb/ccc/dddd",
        "a/bb/ccc/dddd/eeeee",
    ] {
        chmod(&root.join(d), 0o755);
    }
    chmod(&root, 0o755);
    assert_full_parity_default("nested_5_levels", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_unicode_space_symbol_filenames() {
    // FIXTURE: unicode + space + symbol filenames. Pins that the path column survives
    // byte-for-byte (no escaping divergence between facade and oracle) AND the
    // `sort -k5` ordering over multibyte path bytes.
    let root = temp_dir("unicode");
    for name in [
        "café.txt",
        "naïve файл.dat",
        "with space.txt",
        "sym+bol&(name).bin",
        "emoji-🦀.rs",
        "tabless\u{2028}line.txt",
    ] {
        fs::write(root.join(name), name.as_bytes()).unwrap();
        chmod(&root.join(name), 0o644);
    }
    chmod(&root, 0o755);
    assert_full_parity_default("unicode_space_symbol", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_symlinks_to_file_and_dir_default_follow() {
    // FIXTURE: symlink -> file AND symlink -> dir, DEFAULT (follow) mode. The oracle
    // walks `find -L` (follows), so the facade default MUST resolve targets identically.
    let root = temp_dir("symlink-follow");
    fs::create_dir(root.join("realdir")).unwrap();
    fs::write(root.join("realdir/inner.txt"), b"inner").unwrap();
    fs::write(root.join("target.txt"), b"target-bytes").unwrap();
    symlink("target.txt", root.join("link-to-file")).unwrap();
    symlink("realdir", root.join("link-to-dir")).unwrap();
    chmod(&root.join("realdir/inner.txt"), 0o644);
    chmod(&root.join("target.txt"), 0o644);
    chmod(&root.join("realdir"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions::default();
    assert_manifest_parity("symlink_follow", &root, &opts, &[]);
    assert_id_parity("symlink_follow", &root, &opts, &[]);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_symlinks_no_follow() {
    // FIXTURE: same symlink tree, `--no-follow` (`find` not `find -L`). The facade's
    // `ManifestOptions{no_follow:true}` MUST match the oracle `manifest --no-follow`
    // byte-for-byte (symlinks recorded as links, not their targets).
    let root = temp_dir("symlink-nofollow");
    fs::create_dir(root.join("realdir")).unwrap();
    fs::write(root.join("realdir/inner.txt"), b"inner").unwrap();
    fs::write(root.join("target.txt"), b"target-bytes").unwrap();
    symlink("target.txt", root.join("link-to-file")).unwrap();
    symlink("realdir", root.join("link-to-dir")).unwrap();
    chmod(&root.join("realdir/inner.txt"), 0o644);
    chmod(&root.join("target.txt"), 0o644);
    chmod(&root.join("realdir"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions {
        no_follow: true,
        ..ManifestOptions::default()
    };
    assert_manifest_parity("symlink_no_follow", &root, &opts, &["--no-follow"]);
    assert_id_parity("symlink_no_follow", &root, &opts, &["--no-follow"]);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_identical_content_dedup() {
    // FIXTURE: multiple files with IDENTICAL content. Pins that duplicate child
    // checksums collapse (`sort -u`) in the directory merkle exactly as the oracle —
    // every duplicate F line still appears, but the root D-line checksum dedups.
    let root = temp_dir("dedup");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(root.join(name), b"same-content\n").unwrap();
        chmod(&root.join(name), 0o644);
    }
    // one distinct file so the root isn't entirely degenerate
    fs::write(root.join("z-different.txt"), b"unique\n").unwrap();
    chmod(&root.join("z-different.txt"), 0o644);
    chmod(&root, 0o755);
    assert_full_parity_default("identical_content_dedup", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_mixed_permissions() {
    // FIXTURE: mixed file modes 0644/0600/0755 + a 0700 dir. Pins that the PERMISSIONS
    // column (octal) renders byte-identically to the oracle for each entry.
    let root = temp_dir("perms");
    fs::write(root.join("readable.txt"), b"r").unwrap();
    fs::write(root.join("private.txt"), b"p").unwrap();
    fs::write(root.join("executable.sh"), b"#!/bin/sh\n").unwrap();
    fs::create_dir(root.join("lockeddir")).unwrap();
    fs::write(root.join("lockeddir/inner"), b"i").unwrap();
    chmod(&root.join("readable.txt"), 0o644);
    chmod(&root.join("private.txt"), 0o600);
    chmod(&root.join("executable.sh"), 0o755);
    chmod(&root.join("lockeddir/inner"), 0o600);
    chmod(&root.join("lockeddir"), 0o700);
    chmod(&root, 0o755);

    let opts = ManifestOptions::default();
    assert_manifest_parity("mixed_permissions", &root, &opts, &[]);
    assert_id_parity("mixed_permissions", &root, &opts, &[]);

    // Belt-and-suspenders: assert the distinct octal perms actually surfaced (proves
    // the fixture exercised the perm column, not just that two equal blobs matched).
    let oracle = oracle_stdout(&["manifest", &root.to_string_lossy()], &[]);
    assert!(
        oracle.contains(" 600 "),
        "expected a 0600 entry in {oracle}"
    );
    assert!(
        oracle.contains(" 755 "),
        "expected a 0755 entry in {oracle}"
    );
    assert!(
        oracle.contains(" 700 "),
        "expected the 0700 dir in {oracle}"
    );
    fs::remove_dir_all(&root).ok();
}

// --- Option-variant manifest parity ----------------------------------------

#[test]
fn parity_manifest_absolute_paths() {
    // FIXTURE: basic tree rendered with `--absolute` — the facade
    // `ManifestOptions{absolute:true}` must reproduce the oracle's absolute PATH
    // column (root => `<dir>/`, children => `<dir>/<tail>`) byte-for-byte.
    let root = temp_dir("absolute");
    fs::write(root.join("a.txt"), b"hello").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.txt"), b"world!!").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root.join("sub/b.txt"), 0o644);
    chmod(&root.join("sub"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions {
        absolute: true,
        ..ManifestOptions::default()
    };
    assert_manifest_parity("manifest_absolute", &root, &opts, &["--absolute"]);
    assert_id_parity("manifest_absolute", &root, &opts, &["--absolute"]);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_manifest_exclude() {
    // FIXTURE: basic tree with `--exclude sub` — the facade
    // `ManifestOptions{exclude:["sub"]}` must drop the `./sub/` subtree AND recompute
    // the root D-line checksum/size identically to the oracle.
    let root = temp_dir("exclude");
    fs::write(root.join("a.txt"), b"hello").unwrap();
    fs::write(root.join("empty"), b"").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.txt"), b"world!!").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root.join("empty"), 0o644);
    chmod(&root.join("sub/b.txt"), 0o644);
    chmod(&root.join("sub"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions {
        exclude: vec!["sub".to_string()],
        ..ManifestOptions::default()
    };
    assert_manifest_parity("manifest_exclude", &root, &opts, &["--exclude", "sub"]);
    assert_id_parity("manifest_exclude", &root, &opts, &["--exclude", "sub"]);

    // The excluded subtree must be gone on BOTH sides.
    let api = snapdir_api::manifest(&root, &opts).unwrap();
    assert!(
        !api.raw.contains("/sub/"),
        "api manifest must exclude ./sub/"
    );
    fs::remove_dir_all(&root).ok();
}

// --- Alternate-checksum manifest parity + BLAKE3-id invariant ---------------
//
// The CLI `manifest --checksum-bin md5sum|sha256sum` renders the CHECKSUM column
// with that algorithm (cli.rs:3405 dispatches Md5Hasher/Sha256Hasher), but the
// SNAPSHOT-ID is ALWAYS BLAKE3 of the manifest text — `snapdir id` has no
// `--checksum-bin` flag (it only flattens WalkArgs) and the api `id()` hardcodes
// Blake3Hasher regardless of `opts.checksum_bin`. These tests pin BOTH halves so
// a binding that mis-wires `--checksum-bin` (e.g. lets it leak into the id) is
// caught: (a) the alternate-checksum MANIFEST is byte-identical to the oracle,
// and (b) the id stays the BLAKE3-of-that-manifest exactly as `manifest | id`.

/// Asserts that for an alternate `--checksum-bin`, the api manifest is byte-identical
/// to the oracle AND the snapshot id is BLAKE3 of the manifest text (i.e. identical
/// to piping that exact manifest through `snapdir id`). The id therefore VARIES with
/// the checksum algo only because the manifest TEXT (its checksum column) varies —
/// never because `id` consulted the algorithm.
fn assert_alt_checksum_parity(label: &str, fixture: &Path, bin: ChecksumBin, cli_flag: &str) {
    let fixture_str = fixture.to_string_lossy().into_owned();

    // (a) MANIFEST byte-parity under --checksum-bin <flag>.
    let opts = ManifestOptions {
        checksum_bin: bin,
        ..ManifestOptions::default()
    };
    let oracle_manifest =
        oracle_stdout(&["manifest", "--checksum-bin", cli_flag, &fixture_str], &[]);
    let api = snapdir_api::manifest(fixture, &opts)
        .unwrap_or_else(|e| panic!("[{label}] manifest({cli_flag}) failed: {e}"));
    assert_eq!(
        api_raw_normalized(&api.raw),
        oracle_manifest,
        "[{label}] --checksum-bin {cli_flag} manifest must be BYTE-IDENTICAL to the oracle"
    );

    // (b) The id is BLAKE3 of THIS manifest text — equals `manifest --checksum-bin | id`.
    let oracle_id = oracle_id_from_manifest_text(&oracle_manifest);
    let oracle_id = oracle_id.trim_end();
    let api_id = snapdir_api::id(fixture, &opts)
        .unwrap_or_else(|e| panic!("[{label}] id({cli_flag}) failed: {e}"));
    assert_eq!(
        api_id.to_hex(),
        oracle_id,
        "[{label}] id() must be BLAKE3-of-the-manifest (== `manifest --checksum-bin {cli_flag} | id`)"
    );

    // (c) id_from_manifest(manifest(..)) reproduces it from the typed manifest too.
    let from_manifest = snapdir_api::id_from_manifest(&api);
    assert_eq!(
        from_manifest.to_hex(),
        oracle_id,
        "[{label}] id_from_manifest(manifest({cli_flag})) must equal the oracle BLAKE3 id"
    );
}

#[test]
fn parity_checksum_bin_md5_manifest_and_blake3_id() {
    // FIXTURE: a small tree rendered with `--checksum-bin md5sum`. The checksum COLUMN
    // is md5 (32 hex), byte-identical to the oracle; the snapshot id stays BLAKE3.
    let root = temp_dir("md5");
    fs::write(root.join("a.txt"), b"hello md5 world").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.txt"), b"another file").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root.join("sub/b.txt"), 0o644);
    chmod(&root.join("sub"), 0o755);
    chmod(&root, 0o755);
    assert_alt_checksum_parity("md5sum", &root, ChecksumBin::Md5sum, "md5sum");

    // Cross-check: the md5 id MUST DIFFER from the default-BLAKE3 id (proves the
    // manifest TEXT changed — the checksum column really is md5, not BLAKE3).
    let md5_id = snapdir_api::id(
        &root,
        &ManifestOptions {
            checksum_bin: ChecksumBin::Md5sum,
            ..ManifestOptions::default()
        },
    )
    .unwrap();
    let default_id = snapdir_api::id(&root, &ManifestOptions::default()).unwrap();
    assert_ne!(
        md5_id.to_hex(),
        default_id.to_hex(),
        "md5 manifest text differs from BLAKE3 manifest text, so its (BLAKE3-of-text) id must differ"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_checksum_bin_sha256_manifest_and_blake3_id() {
    // FIXTURE: same shape with `--checksum-bin sha256sum`. Pins the sha256 checksum
    // column byte-identically + the BLAKE3-of-manifest id invariant.
    let root = temp_dir("sha256");
    fs::write(root.join("a.txt"), b"hello sha world").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.txt"), b"another file").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root.join("sub/b.txt"), 0o644);
    chmod(&root.join("sub"), 0o755);
    chmod(&root, 0o755);
    assert_alt_checksum_parity("sha256sum", &root, ChecksumBin::Sha256sum, "sha256sum");
    fs::remove_dir_all(&root).ok();
}

// --- Deeper nesting + harder filenames + multi-pattern exclude --------------

#[test]
fn parity_deep_nesting_eight_levels_many_siblings() {
    // FIXTURE: 8 levels deep with sibling files at several levels and duplicate
    // content interleaved — stresses the recursive merkle, `sort -k5` ordering over
    // a deeper tree, and dedup at multiple depths simultaneously.
    let root = temp_dir("deep8");
    let deep = root.join("l1/l2/l3/l4/l5/l6/l7/l8");
    fs::create_dir_all(&deep).unwrap();
    // siblings + duplicate content at varying depths
    fs::write(deep.join("bottom.txt"), b"bottom").unwrap();
    fs::write(root.join("l1/l2/l3/dup.txt"), b"shared-bytes\n").unwrap();
    fs::write(root.join("l1/l2/dup.txt"), b"shared-bytes\n").unwrap();
    fs::write(root.join("l1/a.txt"), b"a").unwrap();
    fs::write(root.join("l1/z.txt"), b"z").unwrap();
    fs::write(root.join("top.txt"), b"top").unwrap();
    for f in [
        "l1/l2/l3/l4/l5/l6/l7/l8/bottom.txt",
        "l1/l2/l3/dup.txt",
        "l1/l2/dup.txt",
        "l1/a.txt",
        "l1/z.txt",
        "top.txt",
    ] {
        chmod(&root.join(f), 0o644);
    }
    for d in [
        "l1",
        "l1/l2",
        "l1/l2/l3",
        "l1/l2/l3/l4",
        "l1/l2/l3/l4/l5",
        "l1/l2/l3/l4/l5/l6",
        "l1/l2/l3/l4/l5/l6/l7",
        "l1/l2/l3/l4/l5/l6/l7/l8",
    ] {
        chmod(&root.join(d), 0o755);
    }
    chmod(&root, 0o755);
    assert_full_parity_default("deep8", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_hard_filenames_leading_dot_dash_and_collation() {
    // FIXTURE: dotfiles, leading-dash names, names that collate ambiguously, and a
    // deliberately adversarial sort set (digits vs letters vs unicode) — pins that the
    // facade walk's ordering + path column match the oracle's `sort -k5` byte-for-byte.
    let root = temp_dir("hardnames");
    for name in [
        ".hidden",
        ".config.toml",
        "-leading-dash.txt",
        "--double-dash",
        "Zebra.TXT",
        "apple.txt",
        "10-ten.txt",
        "2-two.txt",
        "Ünïcödé-CAPS.dat",
        "trailing.space .txt",
    ] {
        fs::write(root.join(name), name.as_bytes()).unwrap();
        chmod(&root.join(name), 0o644);
    }
    chmod(&root, 0o755);
    assert_full_parity_default("hard_filenames", &root);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_manifest_multi_exclude_patterns() {
    // FIXTURE: MULTIPLE `--exclude` patterns (regex), OR-combined. Pins that the
    // facade's `exclude: vec![..]` reproduces the oracle's combine_excludes OR exactly
    // — both the dropped subtrees and the recomputed root D-line.
    let root = temp_dir("multi-exclude");
    fs::write(root.join("keep.txt"), b"keep").unwrap();
    fs::write(root.join("notes.log"), b"log-bytes").unwrap();
    fs::create_dir(root.join("build")).unwrap();
    fs::write(root.join("build/out.o"), b"obj").unwrap();
    fs::create_dir(root.join("cache")).unwrap();
    fs::write(root.join("cache/x.tmp"), b"tmp").unwrap();
    fs::write(root.join("src.rs"), b"fn main(){}").unwrap();
    chmod(&root.join("keep.txt"), 0o644);
    chmod(&root.join("notes.log"), 0o644);
    chmod(&root.join("build/out.o"), 0o644);
    chmod(&root.join("cache/x.tmp"), 0o644);
    chmod(&root.join("src.rs"), 0o644);
    chmod(&root.join("build"), 0o755);
    chmod(&root.join("cache"), 0o755);
    chmod(&root, 0o755);

    // Two regex patterns: drop `build/` and any `.log` file.
    let opts = ManifestOptions {
        exclude: vec!["build".to_string(), r"\.log$".to_string()],
        ..ManifestOptions::default()
    };
    assert_manifest_parity(
        "multi_exclude",
        &root,
        &opts,
        &["--exclude", "build", "--exclude", r"\.log$"],
    );
    assert_id_parity(
        "multi_exclude",
        &root,
        &opts,
        &["--exclude", "build", "--exclude", r"\.log$"],
    );

    // Both excluded targets gone on the api side; the kept ones survive.
    let api = snapdir_api::manifest(&root, &opts).unwrap();
    assert!(
        !api.raw.contains("/build/"),
        "api must exclude ./build/ subtree"
    );
    assert!(
        !api.raw.contains("notes.log"),
        "api must exclude ./notes.log"
    );
    assert!(api.raw.contains("keep.txt"), "api must keep ./keep.txt");
    assert!(
        api.raw.contains("x.tmp"),
        "non-matching ./cache/x.tmp must survive"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_nested_symlink_chain_no_follow() {
    // FIXTURE: a CHAIN of symlinks (link -> link -> real file) plus a symlink into a
    // nested dir, under `--no-follow`. Pins that the facade records each link as a link
    // (not its resolved target) byte-identically to the oracle's plain `find`.
    let root = temp_dir("symlink-chain");
    fs::create_dir_all(root.join("deep/inner")).unwrap();
    fs::write(root.join("deep/inner/real.txt"), b"real-target-bytes").unwrap();
    // chain: a -> b -> deep/inner/real.txt
    symlink("hop-b", root.join("hop-a")).unwrap();
    symlink("deep/inner/real.txt", root.join("hop-b")).unwrap();
    // a link pointing into a nested directory
    symlink("deep/inner", root.join("link-to-inner")).unwrap();
    chmod(&root.join("deep/inner/real.txt"), 0o644);
    chmod(&root.join("deep/inner"), 0o755);
    chmod(&root.join("deep"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions {
        no_follow: true,
        ..ManifestOptions::default()
    };
    assert_manifest_parity("symlink_chain_nofollow", &root, &opts, &["--no-follow"]);
    assert_id_parity("symlink_chain_nofollow", &root, &opts, &["--no-follow"]);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn parity_absolute_and_exclude_combined_roundtrip() {
    // FIXTURE: `--absolute` AND `--exclude` together, with an explicit triple check
    // that `id_from_manifest(manifest(opts)) == id(opts) == (manifest [opts] | id)`
    // for a NON-DEFAULT option combo (guards the impl's option marshalling end-to-end).
    let root = temp_dir("abs-exclude");
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::create_dir(root.join("keep")).unwrap();
    fs::write(root.join("keep/k.txt"), b"kept").unwrap();
    fs::create_dir(root.join("drop")).unwrap();
    fs::write(root.join("drop/d.txt"), b"dropped").unwrap();
    chmod(&root.join("a.txt"), 0o644);
    chmod(&root.join("keep/k.txt"), 0o644);
    chmod(&root.join("drop/d.txt"), 0o644);
    chmod(&root.join("keep"), 0o755);
    chmod(&root.join("drop"), 0o755);
    chmod(&root, 0o755);

    let opts = ManifestOptions {
        absolute: true,
        exclude: vec!["drop".to_string()],
        ..ManifestOptions::default()
    };
    let cli_args = ["--absolute", "--exclude", "drop"];
    // assert_id_parity already pins id() == (manifest [opts] | id) == id_from_manifest.
    assert_manifest_parity("abs_exclude", &root, &opts, &cli_args);
    assert_id_parity("abs_exclude", &root, &opts, &cli_args);

    // Explicit, standalone round-trip assertion for the non-default combo.
    let m = snapdir_api::manifest(&root, &opts).unwrap();
    let manifest_args = [
        "manifest",
        "--absolute",
        "--exclude",
        "drop",
        &root.to_string_lossy().into_owned(),
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect::<Vec<_>>();
    let manifest_arg_refs: Vec<&str> = manifest_args.iter().map(String::as_str).collect();
    let oracle_manifest = oracle_stdout(&manifest_arg_refs, &[]);
    let oracle_id = oracle_id_from_manifest_text(&oracle_manifest);
    assert_eq!(
        snapdir_api::id_from_manifest(&m).to_hex(),
        oracle_id.trim_end(),
        "id_from_manifest must equal `manifest --absolute --exclude drop | id`"
    );
    assert!(
        !m.raw.contains("/drop/"),
        "api manifest must exclude ./drop/"
    );
    assert!(m.raw.contains("/keep/"), "api manifest must keep ./keep/");
    fs::remove_dir_all(&root).ok();
}

// --- Second oracle: the FROZEN golden constants (binary-independent) --------

#[test]
fn parity_frozen_guide_manifests_second_oracle() {
    // SECOND ORACLE (binary-independent): the exact 1.10.0 manifest bytes pinned as
    // frozen constants in `crates/snapdir-core/tests/compat_golden.rs`. We rebuild the
    // guide's "empty files" tree (bar.txt + foo.txt, both empty, dir 0700, files 0600)
    // and assert the facade reproduces the recorded snapshot id + manifest text. This
    // guards against the binary-invocation path being brittle.
    //
    // Frozen from compat_golden.rs:
    //   EMPTY_FILES_MANIFEST (dir 700, files 600, two empty files af1349b9…)
    //   EMPTY_FILES_SNAPSHOT_ID = c678a299380893769bd7795628b96147229b410a9d5a5b7cae563bcae3c27857
    const EMPTY_FILES_SNAPSHOT_ID: &str =
        "c678a299380893769bd7795628b96147229b410a9d5a5b7cae563bcae3c27857";
    const EMPTY_FILES_MANIFEST: &str = "\
D 700 dba5865c0d91b17958e4d2cac98c338f85cbbda07b71a020ab16c391b5e7af4b 0 ./
F 600 af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262 0 ./bar.txt
F 600 af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262 0 ./foo.txt
";

    let root = temp_dir("frozen-guide");
    fs::write(root.join("bar.txt"), b"").unwrap();
    fs::write(root.join("foo.txt"), b"").unwrap();
    chmod(&root.join("bar.txt"), 0o600);
    chmod(&root.join("foo.txt"), 0o600);
    chmod(&root, 0o700);

    let opts = ManifestOptions::default();
    let api = snapdir_api::manifest(&root, &opts).unwrap();
    assert_eq!(
        api_raw_normalized(&api.raw),
        EMPTY_FILES_MANIFEST,
        "facade manifest must equal the FROZEN 1.10.0 guide manifest bytes"
    );
    let id = snapdir_api::id(&root, &opts).unwrap();
    assert_eq!(
        id.to_hex(),
        EMPTY_FILES_SNAPSHOT_ID,
        "facade id must equal the FROZEN 1.10.0 guide snapshot id"
    );

    // And the binary oracle must agree with BOTH (transitive cross-check).
    let oracle_manifest = oracle_stdout(&["manifest", &root.to_string_lossy()], &[]);
    assert_eq!(
        oracle_manifest, EMPTY_FILES_MANIFEST,
        "binary oracle must also equal the frozen guide manifest"
    );
    fs::remove_dir_all(&root).ok();
}

// ===========================================================================
// DIFF PARITY — `snapdir diff --json` vs `snapdir_api::diff(..)`.
// ===========================================================================
//
// The CLI `diff` compares two SIDES, each a SET of `file://` MANIFEST stores
// (+ optional `--id`); it reads MANIFESTS ONLY. Classification per path:
//   A = added (in TO not FROM), D = deleted (in FROM not TO),
//   M = modified (in BOTH, differing), = / hidden = unchanged (shown only with --all).
// The `--json` payload is a flat array `[{"status":"A","path":"./x"}, …]` (status
// in A|D|M, or `=` with --all). The facade `diff(&DiffOptions{from,to,id,all,
// on_conflict})` returns `Vec<DiffEntry{status:DiffStatus, path}>` with
// `DiffStatus::{Added,Deleted,Modified,Unchanged}` displaying as `A`/`D`/`M`/`=`.
// We assert the (status,path) SET is identical on both sides.

/// Maps a facade `DiffStatus` Display char to the CLI/JSON status letter.
fn api_status_letter(s: &snapdir_api::DiffStatus) -> char {
    // §3 fixes Display as 'A'/'D'/'M'/'='.
    let d = format!("{s}");
    let c = d.chars().next().expect("non-empty status display");
    assert!(
        matches!(c, 'A' | 'D' | 'M' | '='),
        "DiffStatus Display must be one of A/D/M/=, got {d:?}"
    );
    c
}

/// Pushes `leaves` into a FRESH `file://` manifest store, returning
/// `(store_dir, store_url, snapshot_id)`. Each store holds exactly ONE manifest.
fn capture(tag: &str, cache: &Path, leaves: &[(&str, &[u8], u32)]) -> (PathBuf, String, String) {
    let src = temp_dir(&format!("{tag}-src"));
    let store = temp_dir(&format!("{tag}-store"));
    for (rel, bytes, mode) in leaves {
        let p = src.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, bytes).unwrap();
        chmod(&p, *mode);
    }
    // deterministic dir perms
    fn set_dirs(d: &Path) {
        chmod(d, 0o755);
        for e in fs::read_dir(d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                set_dirs(&e.path());
            }
        }
    }
    set_dirs(&src);

    let store_url = format!("file://{}", store.display());
    let mut cmd = Command::new(snapdir_bin());
    let out = cmd
        .args(["push", "--store", &store_url, &src.to_string_lossy()])
        .env("SNAPDIR_CACHE_DIR", cache)
        .env_remove("SNAPDIR_STORE")
        .env_remove("SNAPDIR_OBJECTS_STORE")
        .output()
        .expect("run push");
    assert!(
        out.status.success(),
        "push failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8(out.stdout).unwrap().trim_end().to_owned();
    assert_eq!(id.len(), 64, "snapshot id is 64 hex chars");
    fs::remove_dir_all(&src).ok();
    (store, store_url, id)
}

/// Parses `snapdir diff --json` stdout into a sorted `Vec<(status_letter, path)>`.
/// Dep-free flat-JSON parse (the cli test crate has no serde_json; we keep the same
/// constraint here so the staged file needs only `assert_cmd`).
fn parse_diff_json(json: &str) -> Vec<(char, String)> {
    let trimmed = json.trim();
    assert!(
        trimmed.starts_with('[') && trimmed.ends_with(']'),
        "--json must be a JSON array; got:\n{json}"
    );
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut objs = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut start = None;
    for (i, ch) in inner.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    objs.push(inner[start.take().unwrap()..=i].to_owned());
                }
            }
            _ => {}
        }
    }
    assert_eq!(depth, 0, "unbalanced braces in --json:\n{json}");

    let mut out: Vec<(char, String)> = objs
        .iter()
        .map(|frag| {
            let status = json_field(frag, "status").expect("each entry has a status");
            let path = json_field(frag, "path").expect("each entry has a path");
            let c = status.chars().next().expect("non-empty status");
            (c, path)
        })
        .collect();
    out.sort();
    out
}

/// Extracts the value of a flat `"key":"value"` string field from a JSON fragment.
fn json_field(fragment: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = fragment.find(&needle)? + needle.len();
    let rest = fragment[start..].trim_start();
    let stripped = rest.strip_prefix('"')?;
    let end = stripped.find('"')?;
    Some(stripped[..end].to_owned())
}

/// Builds the facade `(status_letter, path)` set from `snapdir_api::diff(..)`.
fn api_diff_set(opts: &snapdir_api::DiffOptions) -> Vec<(char, String)> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let entries = rt
        .block_on(snapdir_api::diff(opts))
        .expect("snapdir_api::diff failed");
    let mut out: Vec<(char, String)> = entries
        .iter()
        .map(|e| {
            let path = e.path.to_string_lossy().into_owned();
            (api_status_letter(&e.status), path)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn parity_diff_added_deleted_modified() {
    // FIXTURE: FROM = {a.txt, gone.txt, keep.txt}; TO = {a.txt(modified), keep.txt, new.txt}.
    // Expect A ./new.txt, D ./gone.txt, M ./a.txt; keep.txt unchanged (hidden default).
    // The facade diff(from,to) classification MUST equal `snapdir diff --json` exactly.
    let cache = temp_dir("diff-adm-cache");

    let (_fs_dir, from_url, _from_id) = capture(
        "adm-from",
        &cache,
        &[
            ("a.txt", b"original", 0o644),
            ("gone.txt", b"to-be-deleted", 0o644),
            ("keep.txt", b"unchanged", 0o644),
        ],
    );
    let (_ts_dir, to_url, to_id) = capture(
        "adm-to",
        &cache,
        &[
            ("a.txt", b"CHANGED", 0o644),
            ("keep.txt", b"unchanged", 0o644),
            ("new.txt", b"freshly-added", 0o644),
        ],
    );

    // Oracle: each store holds exactly one manifest so no --id pin is needed.
    // `snapdir diff` accepts a single global --id (not per-side); since each
    // store has only one manifest, unioning the whole store is equivalent to
    // pinning by id.
    let oracle = oracle_stdout(
        &["diff", "--json", "--from", &from_url, "--to", &to_url],
        &[("SNAPDIR_CACHE_DIR", &cache.to_string_lossy())],
    );
    let oracle_set = parse_diff_json(&oracle);
    let expected: BTreeSet<(char, String)> = [
        ('A', "./new.txt".to_string()),
        ('D', "./gone.txt".to_string()),
        ('M', "./a.txt".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        oracle_set.iter().cloned().collect::<BTreeSet<_>>(),
        expected,
        "oracle diff classification sanity"
    );

    // Facade: same two sides, same pinned ids.
    let opts = snapdir_api::DiffOptions {
        from: vec![StoreUri::parse(&from_url).expect("from uri")],
        to: vec![StoreUri::parse(&to_url).expect("to uri")],
        id: Some(snapdir_api::SnapshotId::from_hex(&to_id).expect("to id")),
        all: false,
        ..Default::default()
    };
    // NOTE: §5 `DiffOptions{from, to, id, all, on_conflict}` carries a SINGLE `id`
    // applied to pin the manifest selection. If the facade requires a per-side id the
    // impl gate must reconcile (FLAGGED in the handoff); the BEHAVIOR pinned is the
    // A/D/M classification SET, which the impl must preserve regardless of ref-token
    // shape.
    let api_set: BTreeSet<(char, String)> = api_diff_set(&opts).into_iter().collect();
    assert_eq!(
        api_set, expected,
        "facade diff(..) must classify A/D/M identically to the 1.10.0 oracle"
    );

    fs::remove_dir_all(&cache).ok();
}

#[test]
fn parity_diff_identical_trees_empty() {
    // FIXTURE: identical FROM and TO trees => NO differences. Both oracle `--json` and
    // facade diff(..) must yield the EMPTY set (no spurious A/D/M).
    let cache = temp_dir("diff-eq-cache");
    let leaves: &[(&str, &[u8], u32)] = &[("x.txt", b"same", 0o644), ("y.txt", b"same2", 0o644)];
    let (_fd, from_url, _from_id) = capture("eq-from", &cache, leaves);
    let (_td, to_url, to_id) = capture("eq-to", &cache, leaves);

    // Each store holds exactly one manifest; no --id pin needed (union == pin).
    let oracle = oracle_stdout(
        &["diff", "--json", "--from", &from_url, "--to", &to_url],
        &[("SNAPDIR_CACHE_DIR", &cache.to_string_lossy())],
    );
    assert!(
        parse_diff_json(&oracle).is_empty(),
        "identical trees must produce no diff entries; got {oracle}"
    );

    let opts = snapdir_api::DiffOptions {
        from: vec![StoreUri::parse(&from_url).unwrap()],
        to: vec![StoreUri::parse(&to_url).unwrap()],
        id: Some(snapdir_api::SnapshotId::from_hex(&to_id).unwrap()),
        all: false,
        ..Default::default()
    };
    assert!(
        api_diff_set(&opts).is_empty(),
        "facade diff(..) of identical trees must be empty"
    );
    fs::remove_dir_all(&cache).ok();
}

#[test]
fn parity_diff_all_includes_unchanged() {
    // FIXTURE: one changed + one unchanged file, with `--all`. The `=` (Unchanged)
    // classification MUST appear on both oracle and facade with identical (status,path).
    let cache = temp_dir("diff-all-cache");
    let (_fd, from_url, _from_id) = capture(
        "all-from",
        &cache,
        &[("same.txt", b"keep", 0o644), ("c.txt", b"v1", 0o644)],
    );
    let (_td, to_url, to_id) = capture(
        "all-to",
        &cache,
        &[("same.txt", b"keep", 0o644), ("c.txt", b"v2", 0o644)],
    );

    // Each store holds exactly one manifest; no --id pin needed (union == pin).
    let oracle = oracle_stdout(
        &[
            "diff", "--json", "--all", "--from", &from_url, "--to", &to_url,
        ],
        &[("SNAPDIR_CACHE_DIR", &cache.to_string_lossy())],
    );
    let oracle_set: BTreeSet<(char, String)> = parse_diff_json(&oracle).into_iter().collect();
    assert!(
        oracle_set.contains(&('M', "./c.txt".to_string())),
        "oracle --all must mark c.txt modified: {oracle}"
    );
    assert!(
        oracle_set.contains(&('=', "./same.txt".to_string())),
        "oracle --all must mark same.txt unchanged (=): {oracle}"
    );

    let opts = snapdir_api::DiffOptions {
        from: vec![StoreUri::parse(&from_url).unwrap()],
        to: vec![StoreUri::parse(&to_url).unwrap()],
        id: Some(snapdir_api::SnapshotId::from_hex(&to_id).unwrap()),
        all: true,
        ..Default::default()
    };
    let api_set: BTreeSet<(char, String)> = api_diff_set(&opts).into_iter().collect();
    assert_eq!(
        api_set, oracle_set,
        "facade diff(--all) must include Unchanged (=) entries identically to the oracle"
    );
    fs::remove_dir_all(&cache).ok();
}
