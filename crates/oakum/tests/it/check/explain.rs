use super::*;

use support::repo_files::{write_changeset, write_patch_changeset};

/// `core` is patched; `lib`'s `^0.1.0` still admits the patch, so the library
/// stays put, while `cli` is a binary and takes the cascade whatever its range.
fn library_and_binary_on_core(label: &str, config: &str, bump: Option<(&str, &str)>) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"core\", \"lib\", \"cli\"]\n",
    )
    .expect("workspace");
    for (member, entry) in [("core", "lib.rs"), ("lib", "lib.rs"), ("cli", "main.rs")] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).expect("member src");
        let on_core = if member == "core" {
            ""
        } else {
            "\n[dependencies]\ncore = { path = \"../core\", version = \"0.1.0\" }\n"
        };
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{on_core}"
            ),
        )
        .expect("member manifest");
        fs::write(dir.join("src").join(entry), "").expect("member source");
    }
    if let Some((name, level)) = bump {
        write_changeset(&root, name, level);
    }
    commit(&root, "init");
    root
}

fn core_patched(label: &str, config: &str) -> Fixture {
    let root = library_and_binary_on_core(label, config, None);
    write_patch_changeset(&root, "core");
    commit(&root, "bump core");
    root
}

#[test]
fn explain_states_each_cascade_and_each_skip_with_its_range() {
    let root = core_patched("explain-skip", "");

    let (plain_code, plain, _) = oakum_exit(&root, &["check"]);
    assert!(
        !plain.contains("explain:"),
        "off unless asked: a library that correctly skips is not reported: {plain}"
    );

    let (code, out, err) = oakum_exit(&root, &["check", "--explain"]);
    assert_eq!(
        code, plain_code,
        "the trace does not move the verdict: {err}"
    );
    assert!(
        out.contains("bump core (cargo): 0.1.0 → 0.1.1 (patch); intent"),
        "{out}\n{err}"
    );
    assert!(
        out.contains(
            "edge core (cargo) → lib (cargo) (normal as core): if-range-unsatisfied; \
             declared ^0.1.0 → skip (range still satisfied)"
        ),
        "the skip names its range and its reason: {out}\n{err}"
    );
    assert!(
        out.contains("edge core (cargo) → cli (cargo) (normal as core): always → cascade patch"),
        "{out}\n{err}"
    );
}

#[test]
fn a_cascade_onto_an_unversioned_dependent_reads_as_skipped() {
    let root = core_patched("explain-unmanaged", "exclude = [\"cli\"]\n");
    let (_, out, err) = oakum_exit(&root, &["check", "--explain"]);
    assert!(
        out.contains("→ cli (cargo) (normal as core): always → skip (not version-managed)"),
        "{out}\n{err}"
    );
    assert!(!out.contains("bump cli"), "{out}\n{err}");
}

#[test]
fn an_empty_plan_says_so() {
    let root = library_and_binary_on_core("explain-empty", "", None);
    let (_, out, err) = oakum_exit(&root, &["check", "--explain"]);
    assert!(
        out.contains("explain: no package is planned to bump"),
        "{out}\n{err}"
    );
}

/// An install pin is no input to the plan, so its absence refuses the run and
/// still traces it.
#[test]
fn a_refusal_the_plan_does_not_read_still_traces() {
    let root = core_patched("explain-unpinned", "");
    fs::remove_dir_all(root.join(".github")).expect("drop the pin");
    commit(&root, "unpin");
    let (code, out, err) = oakum_exit(&root, &["check", "--explain"]);
    assert_eq!(code, Some(2), "{out}\n{err}");
    assert!(
        out.contains("bump core (cargo): 0.1.0 → 0.1.1 (patch); intent"),
        "{out}\n{err}"
    );
}

/// The bump file names a package the plan cannot version, so there is no plan
/// to trace: that must not read as an empty one.
#[test]
fn a_refused_check_says_it_traced_nothing() {
    let root = library_and_binary_on_core(
        "explain-refused",
        "exclude = [\"cli\"]\n",
        Some(("cli", "patch")),
    );
    let (plain_code, _, plain_err) = oakum_exit(&root, &["check"]);
    let (code, out, err) = oakum_exit(&root, &["check", "--explain"]);
    assert_eq!(code, Some(1), "{out}\n{err}");
    assert_eq!(code, plain_code, "{err}");
    assert_eq!(
        err, plain_err,
        "the refusal is said as plain `check` says it"
    );
    assert!(
        out.contains("explain: not traced, check refused above"),
        "{out}\n{err}"
    );
    assert!(!out.contains("no package is planned"), "{out}\n{err}");
}

#[test]
fn explain_is_text_only() {
    let root = core_patched("explain-json", "");
    let (code, out, err) = oakum_exit(&root, &["check", "--explain", "--json"]);
    assert_eq!(code, Some(2), "{out}\n{err}");
    assert!(err.contains("cannot be used with"), "{err}");
}
