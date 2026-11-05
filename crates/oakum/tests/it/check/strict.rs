use super::*;

#[test]
fn default_check_reports_uncovered_without_failing() {
    let root = repo_with_followup_change("cover-advisory");
    let (ok, stdout, stderr) = checked(&root, &["check", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
}

#[test]
fn a_malformed_bump_file_fails_check_and_is_named() {
    let root = temp_git_repo("broken-md");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = true\nconventional-commits = false\n",
    );
    fs::write(root.join(".changeset/broken.md"), "not a bump file\n").expect("broken");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a malformed bump file must not pass: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("`broken.md` is not a bump file: bump file must start with --- on line 1"),
        "{stderr}"
    );
}

#[test]
fn no_config_is_unverified_and_names_the_commands_that_write_one() {
    let root = temp_git_repo("no-config");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = refused_before_the_report(&root, &["check"]);
    assert!(!ok, "defaults must not pass as verified: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "unverified: `.changeset/_config.toml` not found; run `oakum init` or `oakum migrate`"
        ),
        "{stderr}"
    );
}

#[test]
fn strict_fails_when_a_changed_package_has_no_bump_file() {
    let root = repo_with_followup_change("cover-strict-miss");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "strict must fail when a changed package is uncovered");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
    assert!(
        stderr.contains(
            "add a bump file with `oakum add --packages demo:patch`, or write it from conventional commits with `oakum generate --from HEAD~1`"
        ),
        "the defaults read conventional commits, so `generate` can write it: {stderr}"
    );

    write_pinned_config(&root, BINARY_VERSION, "conventional-commits = false\n");
    let (ok, _, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains(
            "; add a bump file with `oakum add --packages demo:<level>` (or `oakum add --none --packages demo:none` if it should release nothing)"
        ),
        "without conventional commits there is nothing for `generate` to read: {stderr}"
    );
}

/// `generate` writes intent for every commit in the range, so with a bump file
/// already there it would write that package's intent twice; only `add` is
/// named then.
#[test]
fn generate_is_not_named_beside_a_bump_file_it_would_duplicate() {
    let root = temp_git_repo("cover-generate-duplicate");
    cargo_members(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("alpha");
    fs::write(root.join("beta/src/lib.rs"), "// b\n").expect("beta");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: minor\n---\n\nalpha thing\n",
    )
    .expect("bump file");
    commit(&root, "fix: change both");

    let (ok, _, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains(
            "asks for a release; add a bump file with `oakum add --packages beta:patch` (or"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("oakum generate"), "{stderr}");
}

/// The base is pasted into a shell, so one that is not a plain name is quoted.
#[test]
fn the_named_generate_quotes_its_base() {
    let root = temp_git_repo("cover-generate-quoted");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: change demo");

    let (ok, _, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1^{commit}"]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("`oakum generate --from 'HEAD~1^{commit}'`"),
        "{stderr}"
    );
}

/// The `generate` the hint names reads the range `check` read: run as
/// printed, it writes the missing bump file.
#[test]
fn the_named_generate_writes_what_check_found_missing() {
    let root = temp_git_repo("cover-generate-remedy");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: change demo");

    let (ok, _, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "{stderr}");
    let printed = stderr
        .split('`')
        .find(|part| part.starts_with("oakum generate"))
        .unwrap_or_else(|| panic!("no generate named: {stderr}"))
        .to_owned();
    let args: Vec<&str> = printed.split_whitespace().skip(1).collect();
    let generated = oakum(&root).args(&args).output().expect("generate");
    assert!(
        generated.status.success(),
        "`{printed}`: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let (ok, _, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "after `{printed}`: {stderr}");
}

#[test]
fn strict_passes_when_a_bump_file_names_the_package() {
    let root = repo_with_followup_change("cover-strict-hit");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\ndemo: patch\n---\n\ncover\n",
    )
    .expect("bump file");
    commit(&root, "cover the change");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// Nested unmanaged packages keep longest-prefix ownership; filtering the
/// managed set before attribution would steal those paths onto the parent.
#[test]
fn coverage_does_not_attribute_excluded_nested_paths_to_the_parent() {
    let root = temp_git_repo("cover-nested-exclude");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"parent\", \"parent/nested\"]\n",
    )
    .expect("workspace");
    for (dir, name) in [("parent", "parent"), ("parent/nested", "nested")] {
        let path = root.join(dir);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"nested\"]\n");
    commit(&root, "init");
    git(&root, &["tag", "parent/v0.1.0"]);
    fs::write(root.join("parent/nested/src/lib.rs"), "// nested only\n").expect("edit nested");
    commit(&root, "chore: touch nested");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        ok,
        "excluded nested change must not uncover parent: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn coverage_ignores_changes_in_an_excluded_package() {
    let root = temp_git_repo("cover-exclude-package");
    cargo_package(&root, "demo", "0.1.0");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "excluded package must not need coverage: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// `--from <base>` diffs `base...HEAD` — from the merge-base — so a base
/// branch that advanced after the branch point cannot pull its own packages
/// into the branch's coverage. A two-dot diff would (measured mutant).
#[test]
fn coverage_ignores_paths_the_base_changed_after_the_branch_point() {
    let root = temp_git_repo("cover-three-dot");
    let listed = "\"alpha\", \"beta\"";
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{listed}]\n"),
    )
    .expect("workspace");
    for name in ["alpha", "beta"] {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(dir.join("src/lib.rs"), "").expect("lib.rs");
    }
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");

    git(&root, &["switch", "-c", "feature"]);
    fs::write(root.join("alpha/src/lib.rs"), "// branch\n").expect("edit alpha");
    fs::write(
        root.join(".changeset/branch.md"),
        "---\nalpha: patch\n---\n\nbranch work\n",
    )
    .expect("bump file");
    commit(&root, "chore: branch work");

    git(&root, &["switch", "main"]);
    fs::write(root.join("beta/src/lib.rs"), "// base advanced\n").expect("edit beta");
    commit(&root, "chore: base advance");
    git(&root, &["switch", "feature"]);

    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "main"]);
    assert!(
        ok,
        "beta changed only on the advanced base and must not need coverage: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_empty_frontmatter_covers_changed_packages() {
    let root = repo_with_followup_change("cover-empty");
    fs::write(
        root.join(".changeset/empty.md"),
        "---\n---\n\nintentional none\n",
    )
    .expect("empty bump");
    commit(&root, "cover the change");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_none_entry_covers_the_named_package() {
    let root = repo_with_followup_change("cover-none");
    fs::write(
        root.join(".changeset/none.md"),
        "---\ndemo: none\n---\n\ncovered without a release\n",
    )
    .expect("none bump");
    commit(&root, "cover the change");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn tag_drift_skips_coverage() {
    let root = repo_with_followup_change("cover-tag-drift");
    let (ok, stdout, stderr) = oakum_output(&root, &["tag-drift"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_commits_only_intent_covers_without_a_bump_file() {
    let root = temp_git_repo("cover-commits-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        ok,
        "commits-only --strict must treat feat(demo) as covering, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_ignores_commits_when_change_files_are_on() {
    let root = temp_git_repo("cover-files-on-feat");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        !ok,
        "feat(demo) must not cover when change files are on, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
    assert!(stderr.contains("bump file"), "{stderr}");
}

/// A bump file whose note is a heading with nothing under it renders no
/// changelog section, so `version` consumes it and the release says nothing
/// about what its author wrote. `check` reports that always and refuses under
/// `--strict`, which is the shape the coverage look already uses: a repository
/// green today stays green until it opts in.
#[test]
fn a_note_that_would_reach_no_changelog_reports_and_gates_under_strict() {
    let root = temp_git_repo("dropped-note");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join(".changeset/heading-only.md"),
        "---\ndemo: minor\n---\n\n### Added\n",
    )
    .expect("bump file");
    // Two, so the count is the run's rather than a constant that happens to
    // read right.
    fs::write(
        root.join(".changeset/also-dropped.md"),
        "---\ndemo: patch\n---\n\n### Fixed\n",
    )
    .expect("second bump file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, stdout, reported) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "reports without gating: {reported}{stdout}");
    // Beside the coverage look's own report lines, which stderr carries.
    assert!(
        reported.contains("`heading-only.md` has a heading with nothing under it"),
        "{reported}"
    );

    let (strict_code, _, strict_err) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(
        strict_code,
        Some(1),
        "a finding, not an unverified look: {strict_err}"
    );
    assert!(
        strict_err.contains("2 bump file note(s) have a heading that would reach no changelog"),
        "the count is the run's, not a constant: {strict_err}"
    );
    // Naming the file is the look's whole value; the count alone passes with
    // the evidence lines dropped.
    for named in ["`heading-only.md`", "`also-dropped.md`"] {
        assert!(strict_err.contains(named), "{named} unnamed: {strict_err}");
    }
}

/// A note that keeps one section and loses another reaches a changelog, so
/// the release looks complete; the lost heading is what `check` names.
#[test]
fn a_note_that_loses_one_heading_of_several_is_reported() {
    let root = temp_git_repo("partly-dropped-note");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join(".changeset/partial.md"),
        "---\ndemo: minor\n---\n\n### Added\n\nreal text\n\n### Fixed\n",
    )
    .expect("bump file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, _, reported) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{reported}");
    assert!(
        reported.contains("`partial.md` has a heading with nothing under it (`### Fixed`)"),
        "{reported}"
    );
    let (strict_code, _, strict_err) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(strict_code, Some(1), "{strict_err}");
    assert!(
        strict_err.contains("1 bump file note(s) have a heading that would reach no changelog"),
        "{strict_err}"
    );
}

/// `--strict` decides two looks, so the document names them rather than
/// answering yes or no. A bool could not say which of them refused.
#[test]
fn the_document_names_the_looks_strict_gates() {
    let root = temp_git_repo("json-gating");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, document, stderr) = oakum_exit(&root, &["check", "--strict", "--json"]);
    assert_eq!(code, Some(0), "a clean repository: {stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    assert_eq!(
        parsed["scope"]["gating"],
        serde_json::json!(["coverage", "notes"]),
        "{document}"
    );
    // The schema stays at 1: `check --json` has not appeared in a release, so
    // no consumer can be holding the old field name.
    assert_eq!(parsed["schema_version"], 1, "{document}");
}

/// Both gated looks read the plan from a base, so with no base ref neither
/// runs and the sentence has to say so. "We didn't look" printed as "it looked
/// and found nothing" is the collapse this project's invariant names.
#[test]
fn with_no_base_ref_neither_gated_look_is_announced_as_reporting() {
    let root = temp_git_repo("no-base-scope");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");

    let (_, stdout, stderr) = oakum_exit(&root, &["check", "--from", "no-such-ref"]);
    let said = format!("{stdout}{stderr}");
    assert!(
        said.contains("coverage and notes cannot run without a base ref"),
        "both are named, not just coverage: {said}"
    );
    assert!(
        !said.contains("without gating"),
        "a look that cannot run must not be announced as reporting: {said}"
    );

    // The document says what `--strict` decided, which the prose deliberately
    // does not: an empty list here could not distinguish this run from one
    // where `--strict` was never passed, and each look's own row already
    // carries `unverified`.
    let (_, document, _) = oakum_exit(
        &root,
        &["check", "--strict", "--from", "no-such-ref", "--json"],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    assert_eq!(
        parsed["scope"]["gating"],
        serde_json::json!(["coverage", "notes"]),
        "{document}"
    );
    let notes = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "notes")
        .expect("a notes row");
    assert_eq!(notes["outcome"], "unverified", "{document}");
}

/// The two shapes that render nothing on purpose. Gating these would refuse
/// every releaseless round and every bump file nobody wrote a note in.
#[test]
fn a_coverage_only_note_and_a_noteless_bump_file_do_not_gate() {
    let root = temp_git_repo("not-dropped");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    std::fs::write(
        root.join(".changeset/coverage-only.md"),
        "---\ndemo: none\n---\n\n### Added\n",
    )
    .expect("coverage bump file");
    std::fs::write(
        root.join(".changeset/no-note.md"),
        "---\ndemo: patch\n---\n",
    )
    .expect("noteless bump file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(code, Some(0), "{stderr}{stdout}");
    assert!(!stdout.contains("nothing under it"), "{stdout}");
    assert!(!stderr.contains("nothing under it"), "{stderr}");
}
