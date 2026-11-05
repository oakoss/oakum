//! The integration tests build as one binary, so a suite exists only while
//! `main.rs` declares it and something in it registers a test.

use crate::support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Outside `it/`, a `.rs` file below `tests/` builds as its own binary or as
/// nothing; below `fixtures/` only its `main.rs` does, the rest being
/// Cargo-crate sources. A `tests/it/` file this binary skips runs none of its
/// tests. Every file must register a test in this binary's `--list` unless
/// `TEST_FREE` names it.
#[test]
fn every_suite_compiles_into_the_one_test_binary() {
    let tests = support::workspace_root().join("crates/oakum/tests");
    let root = tests.join("it");
    let fixtures = tests.join("fixtures");
    let outside: Vec<PathBuf> = support::rust_files_below(&tests, &["it/main.rs"])
        .into_iter()
        .filter(|path| !hidden(&tests, path))
        .filter(|path| {
            !path.starts_with(&root)
                && (!path.starts_with(&fixtures) || *path == fixtures.join("main.rs"))
        })
        .collect();
    assert!(
        outside.is_empty(),
        "each builds as its own test binary or as nothing; make it a module of tests/it/main.rs instead: {outside:?}"
    );

    let on_disk: BTreeSet<String> = support::rust_files_below(&root, &["main.rs"])
        .iter()
        .filter(|path| !hidden(&root, path))
        .map(|path| relative(&root, path))
        .collect();
    let exe = std::env::current_exe().expect("the test executable's path");
    let listing = std::process::Command::new(&exe)
        .arg("--list")
        .output()
        .unwrap_or_else(|e| panic!("{} --list should run: {e}", exe.display()));
    assert!(
        listing.status.success(),
        "{} --list exited {}: {}",
        exe.display(),
        listing.status,
        String::from_utf8_lossy(&listing.stderr)
    );
    assert_eq!(
        files_registering_no_test(&on_disk, &String::from_utf8_lossy(&listing.stdout)),
        TEST_FREE.into_iter().collect::<BTreeSet<_>>(),
        "the files below {} that register no test in {} must be exactly TEST_FREE. A suite \
         must compile a test on every target CI builds; a file-level #![cfg] however spelled, \
         every test gated off for this target, a missing mod line, or a module path that \
         differs from its file drops them all",
        root.display(),
        exe.display()
    );
}

/// Files below `tests/it/` that hold no tests by design. Named rather than
/// inferred from their text: a test spelled any other way would read as none.
const TEST_FREE: [&str; 7] = [
    "check/mod.rs",
    "main.rs",
    "support/fixture.rs",
    "support/mod.rs",
    "support/repo_files.rs",
    "support/repo_state.rs",
    "support/tools.rs",
];

/// Each name in libtest's `--list` output counts for the file whose module path
/// is its longest prefix, so a `mod.rs` gets no credit for its children's tests.
fn files_registering_no_test<'a>(on_disk: &'a BTreeSet<String>, listed: &str) -> BTreeSet<&'a str> {
    let prefixes: Vec<(&str, String)> = on_disk
        .iter()
        .map(|file| (file.as_str(), module_prefix(file)))
        .collect();
    let registering: BTreeSet<&str> = listed
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .filter_map(|name| {
            prefixes
                .iter()
                .filter(|(_, prefix)| name.starts_with(prefix.as_str()))
                .max_by_key(|(_, prefix)| prefix.len())
                .map(|(file, _)| *file)
        })
        .collect();
    on_disk
        .iter()
        .map(String::as_str)
        .filter(|file| !registering.contains(file))
        .collect()
}

fn module_prefix(relative: &str) -> String {
    if relative == "main.rs" {
        return String::new();
    }
    let module = relative.strip_suffix(".rs").unwrap_or(relative);
    let module = module.strip_suffix("/mod").unwrap_or(module);
    format!("{}::", module.replace('/', "::"))
}

/// Shapes `files_registering_no_test` must tell apart: a name that only
/// contains another module's path, a `mod.rs` whose children hold the tests,
/// a nested test module, a benchmark line, and names only the crate root owns.
#[test]
fn a_listed_test_counts_for_the_file_whose_module_path_is_its_longest_prefix() {
    let on_disk: BTreeSet<String> = [
        "main.rs",
        "cli.rs",
        "add_cli.rs",
        "gated.rs",
        "support/mod.rs",
        "support/foreign.rs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let listed = "add_cli::a: test\n\
                  root_test: test\n\
                  gone::cli::e: test\n\
                  support::foreign::b: test\n\
                  support::foreign::tests::c: test\n\
                  gated::timing: bench\n\
                  \n\
                  5 tests, 1 benchmark\n";
    assert_eq!(
        files_registering_no_test(&on_disk, listed),
        ["cli.rs", "gated.rs", "support/mod.rs"]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
}

/// Cargo discovers no target through a dot-named file or directory, and
/// editors leave such files (`.#x.rs` lock links, macOS `._x.rs`).
fn hidden(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .expect("the walk stays below its root")
        .components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
}

/// `path` below `root`, with forward slashes on every platform.
fn relative(root: &Path, path: &Path) -> String {
    let parts: Vec<_> = path
        .strip_prefix(root)
        .expect("the walk stays below its root")
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    parts.join("/")
}
