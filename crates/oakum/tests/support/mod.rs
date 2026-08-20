//! Shared by the tests that read repository files rather than this crate's API.
//!
//! Each integration test is its own crate and compiles this module whole, so a
//! helper only some of them call is dead code in the rest. `expect` cannot be
//! used here for the same reason — it would go unfulfilled in the crates that do
//! call it.
#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};

/// Cargo's glob metacharacters. A member list using any of them resolves to
/// packages this module cannot name, so it is refused rather than guessed at.
const GLOB_CHARS: [char; 3] = ['*', '?', '['];

pub fn workspace_root() -> PathBuf {
    workspace().0
}

/// The workspace root and every member directory in it, both absolute.
///
/// The root is reached by a fixed climb out of this package rather than by
/// searching upward for a marker file. A search accepts whatever happens to sit
/// above the checkout — a stray `clippy.toml` in a parent directory, another
/// project's `Cargo.toml` — and reports success either way; the climb is right or
/// it fails here.
///
/// The assertion is what makes it checkable rather than a guess: the manifest it
/// lands on must be the workspace root that lists this package as a member. Move
/// the crate and this fails, instead of quietly pointing every caller at an
/// unrelated directory.
pub fn workspace() -> (PathBuf, Vec<PathBuf>) {
    #[derive(serde::Deserialize)]
    struct Manifest {
        workspace: Workspace,
    }

    #[derive(serde::Deserialize)]
    struct Workspace {
        members: Vec<String>,
    }

    // Left un-canonicalized on purpose: `io_boundary.rs` walks `ancestors()` down
    // to this value, and that loop terminates on string equality. Resolving
    // symlinks here would make it miss and scan every ancestor to `/`.
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = package
        .ancestors()
        .nth(2)
        .expect("this package should sit two levels below the workspace root")
        .to_path_buf();

    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .unwrap_or_else(|e| panic!("{}/Cargo.toml should be readable: {e}", root.display()));
    // `expect` would render the error with `Debug`, which embeds the whole file.
    let manifest: Manifest = toml::from_str(&manifest).unwrap_or_else(|e| {
        panic!(
            "{}/Cargo.toml is not a workspace root manifest: {e}",
            root.display()
        )
    });

    // Checked before the membership assertion below, so a glob list reports what
    // it is instead of reporting "not a member" about a package cargo resolves.
    assert!(
        !manifest
            .workspace
            .members
            .iter()
            .any(|member| member.contains(GLOB_CHARS)),
        "{}/Cargo.toml lists members by glob, which this anchor cannot expand — \
         list them literally, or teach every caller of this module to expand globs",
        root.display()
    );

    // `..` and an absolute entry both survive `join` lexically, and every
    // comparison downstream is lexical — `crates/../crates/plan-no-std` resolves
    // for cargo while making the shadow scan in `io_boundary.rs` accuse the root's
    // own config.
    assert!(
        !manifest.workspace.members.iter().any(|member| {
            Path::new(member)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        }),
        "{}/Cargo.toml lists a member by a path this anchor compares lexically \
         (`..`, or an absolute path) — list members as plain relative paths",
        root.display()
    );

    // Joined onto the root rather than compared as strings, so a separator or a
    // `./` prefix does not decide whether a member matches.
    let members: Vec<PathBuf> = manifest
        .workspace
        .members
        .iter()
        .map(|member| root.join(member))
        .collect();

    assert!(
        members.contains(&package.to_path_buf()),
        "{} does not list {} as a member, so it is not this package's workspace root",
        root.display(),
        package.display()
    );

    (root, members)
}

/// The commands one `.mise.toml` task runs, whether its `run` is a single string
/// or an array. Several tests assert flags on these; a task whose shape changed
/// must fail rather than read as having no commands.
pub fn task_commands<'a>(mise: &'a toml::Value, task: &str) -> Vec<&'a str> {
    // Indexed with `get`, whose absence this names: `[]` panics `index not found`
    // and says neither which task nor which file.
    let run = mise
        .get("tasks")
        .and_then(|tasks| tasks.get(task))
        .and_then(|task| task.get("run"))
        .unwrap_or_else(|| panic!(".mise.toml declares no [tasks.{task}] with a `run`"));

    match run {
        toml::Value::String(one) => Vec::from([one.as_str()]),
        toml::Value::Array(many) => many.iter().filter_map(toml::Value::as_str).collect(),
        other => panic!("[tasks.{task}].run is neither a string nor an array: {other:?}"),
    }
}

/// Comment-stripped source lines. A commented-out declaration, or a doc comment
/// quoting one, satisfies a `contains` check while the thing it names is gone.
///
/// Only line comments are stripped. A block comment is refused rather than
/// mishandled: its delimiters survive as ordinary lines, so the code between
/// them would read as live.
pub fn code_lines(source: &str, marker: &str) -> Vec<String> {
    assert!(
        !source.contains("/*"),
        "this source carries a block comment, which this stripper does not \
         understand — the lines inside one would read as live code"
    );

    source
        .lines()
        .filter_map(|line| {
            let code = line.split(marker).next().unwrap_or_default().trim();
            (!code.is_empty()).then(|| code.to_string())
        })
        .collect()
}
