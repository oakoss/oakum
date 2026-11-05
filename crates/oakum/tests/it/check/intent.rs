use super::*;

/// `git status` warns and continues on a directory it cannot open: exit 0, a
/// short listing, and the warning on stderr. Read by emptiness alone that is a
/// complete tree, so the package whose only change lived under that directory
/// is passed over in silence — the defect this whole look exists to close,
/// one layer down.
///
/// Measured before the wholeness rule landed: with `beta/hidden` unreadable,
/// `alpha` was still named and `beta` simply vanished, with nothing said about
/// the warning git had written.
///
/// The two members are load-bearing twice over. They are what makes the
/// listing partial rather than empty, and they are the only thing pinning `-z`
/// on this op: without it the whole listing arrives as one record whose path
/// opens with `.changeset/`, so the intent filter discards the tree and no
/// package is ever named. Reducing this to one member would silently retire
/// both claims.
#[cfg(unix)]
#[test]
fn a_partial_worktree_listing_is_not_read_as_a_whole_one() {
    let root = temp_git_repo("outside-head-partial");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    // Both members managed: an unmanaged one never reaches coverage, so it
    // could not show the difference between seeing it and missing it.
    for member in ["alpha", "beta"] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).expect("member src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member manifest");
        fs::write(dir.join("src/lib.rs"), "").expect("member lib");
    }
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("alpha/src/lib.rs"), "// visible\n").expect("visible edit");
    let hidden = root.join("beta/hidden");
    fs::create_dir_all(&hidden).expect("hidden dir");
    fs::write(hidden.join("new.rs"), "// buried\n").expect("buried edit");

    let (whole, _, readable) = oakum_exit(&root, &["check", "--from", "v0.1.0"]);
    assert!(
        readable.contains("alpha (cargo): changed outside `HEAD`")
            && readable.contains("beta (cargo): changed outside `HEAD`"),
        "control: both are named while the whole tree can be read: {readable}"
    );

    let (code, _, sealed) = {
        let _sealed = support::fixture::Sealed::set(&hidden, 0o000);
        oakum_exit(&root, &["check", "--from", "v0.1.0"])
    };

    // Against the control's own code, not a literal: this fixture also carries
    // an unrelated `leftover tag ambiguity` look, and asserting `Some(2)` here
    // passed on the strength of that instead of on anything under test.
    assert_eq!(
        code, whole,
        "the worktree half is advisory in both directions, so it does not move \
         the exit outcome: {sealed}"
    );
    assert!(
        sealed.contains("may have listed only part of the tree"),
        "and it says what it could not read: {sealed}"
    );
    assert!(
        !sealed.contains("beta (cargo): changed outside `HEAD`"),
        "and it must not claim to have seen what it could not read: {sealed}"
    );
}

/// At mode 444 the directory still lists but its entries cannot be `lstat`ed,
/// so `status` drops a modified tracked file through a bare `perror` line with
/// no `warning:` (measured, git 2.55). Root reads through the mode, so the case
/// runs only where it holds.
#[cfg(unix)]
#[test]
fn a_worktree_listing_shortened_by_an_unprefixed_line_is_not_whole() {
    let root = temp_git_repo("outside-head-unprefixed");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for member in ["alpha", "beta"] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).expect("member src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member manifest");
        fs::write(dir.join("src/lib.rs"), "").expect("member lib");
    }
    // One level below `src`, so `cargo metadata` can still find the target.
    fs::create_dir_all(root.join("beta/src/inner")).expect("inner");
    fs::write(root.join("beta/src/inner/mod.rs"), "").expect("inner file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("alpha/src/lib.rs"), "// visible\n").expect("visible edit");
    fs::write(root.join("beta/src/inner/mod.rs"), "// buried\n").expect("buried edit");

    let inner = root.join("beta/src/inner");
    let (enforced, sealed) = {
        let _sealed = support::fixture::Sealed::set(&inner, 0o444);
        let enforced = fs::metadata(inner.join("mod.rs")).is_err();
        (
            enforced,
            oakum_exit(&root, &["check", "--from", "v0.1.0"]).2,
        )
    };

    if !enforced {
        support::skip("mode 444 does not stop an lstat here (running as root?)");
        return;
    }
    assert!(
        sealed.contains("may have listed only part of the tree")
            && sealed.contains("Permission denied"),
        "{sealed}"
    );
    assert!(
        !sealed.contains("beta (cargo): changed outside `HEAD`"),
        "{sealed}"
    );
}

/// `check` is pure, and the index is the file this look could most easily
/// write: `git status` refreshes stale stat information and rewrites
/// `.git/index` unless told not to. Measured before `--no-optional-locks`
/// landed — one `check` moved the index bytes on a tree whose content had not
/// changed at all.
///
/// Not covered by `check_leaves_repository_state_unchanged`: `RepoState`
/// ignores the index deliberately, which
/// `repo_state_rejects_staging_without_worktree_byte_change` establishes, so
/// this invariant is unguarded anywhere else.
#[test]
fn check_does_not_rewrite_the_index() {
    let root = temp_git_repo("index-purity");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    // Same bytes, a distinctly older mtime. Rewriting the file with identical
    // content is not enough: an mtime equal to the index's is "racily clean"
    // to git, which re-hashes and finds nothing to record. A stat that plainly
    // disagrees is what makes `status` want to refresh.
    let manifest = root.join("Cargo.toml");
    let handle = fs::File::options()
        .write(true)
        .open(&manifest)
        .expect("open manifest");
    let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    handle
        .set_times(fs::FileTimes::new().set_accessed(past).set_modified(past))
        .expect("age the manifest");
    drop(handle);
    let index = root.join(".git/index");
    let before = fs::read(&index).expect("read index");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        fs::read(&index).expect("read index"),
        before,
        "`check` reports drift; it does not write to the repository it reads"
    );
}

/// The mirror of `a_change_outside_head_is_named_rather_than_passed_over`:
/// there the change is invisible, here the coverage is. Intent is read off
/// disk, so a bump file that is not in a commit covers a change that is —
/// measured on 0.3.1 and again on the commit before this one: the same base and
/// the same committed change exited 1 with `1 package(s) changed with no
/// covering intent`, and writing one untracked file under `.changeset/` made it
/// exit 0 saying nothing. Nothing about that file reaches a pull request, so
/// the ref the author pushes is the one that exited 1.
///
/// Four states against one base, because the defect is the difference: the
/// file absent, present but uncommitted, committed, and committed then edited.
#[test]
fn intent_head_does_not_carry_is_named_rather_than_counted_in_silence() {
    let root = temp_git_repo("off-disk-intent");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    let off_disk = "demo (cargo): covered on disk by `.changeset/cover.md`, not by `HEAD`";

    let (code, _, bare) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(1), "no intent at all is the finding: {bare}");
    assert!(
        bare.contains("1 package(s) changed with no covering intent"),
        "{bare}"
    );

    fs::write(
        root.join(".changeset/cover.md"),
        "---\ndemo: patch\n---\nnote\n",
    )
    .expect("bump");
    let (code, _, uncommitted) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(
        code,
        Some(0),
        "advisory, like the rest of this half — it names what a pull request \
         will not get, it does not refuse: {uncommitted}"
    );
    assert!(uncommitted.contains(off_disk), "{uncommitted}");

    commit(&root, "chore: cover demo");
    let (code, _, committed) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{committed}");
    assert!(
        !committed.contains("not by `HEAD`"),
        "once committed there is nothing to say: {committed}"
    );

    // A fourth state, because the third cannot fail the way this one can: a
    // committed tree is clean, so `git status` lists nothing and a rule that
    // read no further would still look right. Rewording puts the file back in
    // the listing while leaving what `HEAD` covers untouched.
    fs::write(
        root.join(".changeset/cover.md"),
        "---\ndemo: patch\n---\nnote, reworded\n",
    )
    .expect("reword");
    let (code, _, edited) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{edited}");
    assert!(
        !edited.contains("not by `HEAD`"),
        "`HEAD`'s copy still covers this package, so the reword changes no answer: {edited}"
    );
}

/// Two members, both managed, so coverage has something to decide about each.
/// A root package would own every path and make "which package" vacuous.
fn two_managed_members(label: &str) -> (Fixture, String) {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for member in ["alpha", "beta"] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).expect("member src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member manifest");
        fs::write(dir.join("src/lib.rs"), "").expect("member lib");
    }
    commit(&root, "init");
    // The base as a commit, not a tag: a bare `v0.1.0` over members that are
    // themselves 0.1.0 is what the tag look calls ambiguous, and that refusal
    // would decide the exit code these tests are about.
    let base = head_sha(&root);
    (root, base)
}

/// The case that made path-presence the wrong question. A bump file committed
/// covering one package, then edited on disk to name a second, covers that
/// second package here while `HEAD` covers nothing of the sort — measured
/// before the rewrite: the tree that exits 1 naming `other` went silent the
/// moment the line was added on disk.
///
/// Ordinary, which is what makes it matter: commit a bump file, touch another
/// package later, add a line to the file you already have rather than writing a
/// second one.
#[test]
fn intent_edited_on_disk_to_cover_more_is_named() {
    let (root, base) = two_managed_members("off-disk-edited");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\nalpha: patch\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("alpha");
    fs::write(root.join("beta/src/lib.rs"), "// beta\n").expect("beta");
    commit(&root, "fix: change both, cover alpha");

    let (code, _, committed) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(
        code,
        Some(1),
        "this is what a pull request gets: {committed}"
    );
    assert!(
        committed.contains("beta (cargo): changed with no covering intent"),
        "{committed}"
    );

    // The same file `HEAD` already has, now naming `beta` too — on disk only.
    fs::write(
        root.join(".changeset/cover.md"),
        "---\nalpha: patch\nbeta: patch\n---\nnote\n",
    )
    .expect("reword");
    let (code, _, edited) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{edited}");
    assert!(
        edited.contains("beta (cargo): covered on disk by `.changeset/cover.md`, not by `HEAD`"),
        "the file is in `HEAD`; the coverage is not: {edited}"
    );
}

/// The version commit consumes the bump files that covered its writes, so a
/// range holding only it would otherwise read as an uncovered change whose
/// remedy bumps the package again. Recognised by what it does, not its subject,
/// which `commit-message` sets.
#[test]
fn a_version_commit_is_not_an_uncovered_change() {
    let (root, _) = two_managed_members("version-commit");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nfix alpha\n",
    )
    .expect("bump");
    commit(&root, "chore: add alpha's bump file");
    let before = head_sha(&root);
    let (code, _, versioned) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{versioned}");
    commit(&root, "Ship alpha");
    git(&root, &["tag", "alpha@0.1.1"]);

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(0), "the version commit's own writes: {stderr}");

    // An ordinary commit after it still counts, including one touching a file
    // the version commit wrote.
    fs::write(root.join("alpha/CHANGELOG.md"), "# edited by hand\n").expect("hand edit");
    commit(&root, "fix: edit alpha's changelog");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A hand edit to another package's manifest inside the version commit still
/// counts: `version` wrote no changelog for it.
#[test]
fn a_version_commit_does_not_excuse_another_packages_manifest() {
    let (root, _) = two_managed_members("version-commit-rider");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nfix alpha\n",
    )
    .expect("bump");
    commit(&root, "chore: add alpha's bump file");
    let before = head_sha(&root);
    let (code, _, versioned) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{versioned}");
    let manifest = root.join("beta/Cargo.toml");
    let mut beta = fs::read_to_string(&manifest).expect("beta");
    beta.push_str("\n[features]\nextra = []\n");
    fs::write(&manifest, beta).expect("beta by hand");
    commit(&root, "Ship alpha");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("beta (cargo): changed with no covering intent"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// `git log` detects renames by default; one in the range is still a change,
/// not a listing that could not be read.
#[test]
fn a_rename_in_the_range_is_a_change_not_an_unread_listing() {
    let (root, _) = two_managed_members("version-commit-rename");
    fs::write(root.join("alpha/notes.txt"), "notes\n").expect("notes");
    commit(&root, "fix: alpha notes");
    let before = head_sha(&root);
    git(&root, &["mv", "alpha/notes.txt", "alpha/notes.md"]);
    commit(&root, "fix: rename alpha notes");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A bump file pending on the base branch covers nothing a branch changes; one
/// the branch adds or edits does, and `status` reads coverage the same way.
#[test]
fn only_the_branchs_own_bump_files_cover_it() {
    let (root, _) = two_managed_members("own-intent");
    let pending = root.join(".changeset/pending.md");
    fs::write(&pending, "---\nalpha: patch\n---\nfix alpha\n").expect("bump");
    commit(&root, "chore: alpha's pending bump file");
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha again");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["status", "--json", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        value["uncovered"],
        serde_json::json!([{"ecosystem": "cargo", "name": "alpha"}]),
        "{stdout}"
    );

    fs::write(&pending, "---\nalpha: patch\n---\nfix alpha, twice\n").expect("edit");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): covered on disk by `.changeset/pending.md`, not by `HEAD`"),
        "an uncommitted edit covers here and not on the pull request: {stderr}"
    );

    commit(&root, "chore: alpha's bump file names the second fix");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "an edited file is the branch's: {stderr}");
    let (code, stdout, stderr) = oakum_exit(&root, &["status", "--json", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["uncovered"], serde_json::json!([]), "{stdout}");
}

/// A new bump file covers the branch beside one pending on the base, and a
/// branch that changes nothing has nothing to cover.
#[test]
fn a_bump_file_the_branch_adds_covers_it() {
    let (root, _) = two_managed_members("own-intent-added");
    fs::write(
        root.join(".changeset/pending.md"),
        "---\nalpha: patch\n---\nfix alpha\n",
    )
    .expect("bump");
    commit(&root, "chore: alpha's pending bump file");
    let base = head_sha(&root);
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");

    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha again");
    fs::write(
        root.join(".changeset/again.md"),
        "---\nalpha: patch\n---\nfix alpha again\n",
    )
    .expect("own bump");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): covered on disk by `.changeset/again.md`, not by `HEAD`"),
        "the base's file does not stand in for the uncommitted one: {stderr}"
    );

    commit(&root, "chore: alpha's own bump file");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("covered on disk"), "{stderr}");
}

/// A file's intent is its entries as much as its note: widening a pending file
/// to name the branch's package is the branch's own intent.
#[test]
fn a_pending_file_the_branch_widens_covers_it() {
    let (root, _) = two_managed_members("own-intent-widened");
    let pending = root.join(".changeset/pending.md");
    fs::write(&pending, "---\nalpha: patch\n---\nfix\n").expect("bump");
    commit(&root, "chore: alpha's pending bump file");
    let base = head_sha(&root);
    fs::write(root.join("beta/src/lib.rs"), "// beta\n").expect("change");
    fs::write(&pending, "---\nalpha: patch\nbeta: patch\n---\nfix\n").expect("widen");
    commit(&root, "fix: beta too");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// A new file is the branch's own even when its content matches one pending on
/// the base, as two short `patch` files easily do.
#[test]
fn a_new_file_matching_a_pending_one_covers_it() {
    let (root, _) = two_managed_members("own-intent-twin");
    fs::write(
        root.join(".changeset/a.md"),
        "---\nalpha: patch\n---\nfix\n",
    )
    .expect("bump");
    commit(&root, "chore: alpha's pending bump file");
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    fs::write(
        root.join(".changeset/b.md"),
        "---\nalpha: patch\n---\nfix\n",
    )
    .expect("twin");
    commit(&root, "fix: alpha again");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// A file pending on the base still asks for a release that cannot happen, so
/// it is refused though it covers nothing on this branch.
#[test]
fn a_pending_file_naming_an_excluded_package_is_still_refused() {
    let (root, _) = two_managed_members("own-intent-excluded");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"beta\"]\n");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: patch\n---\nfix\n",
    )
    .expect("bump");
    commit(&root, "chore: beta's pending bump file");
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nfix\n",
    )
    .expect("own");
    commit(&root, "fix: alpha");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("not version-managed"), "{stderr}");
}

/// A shallow base finds no merge-base; the cause named is the fetch depth.
#[test]
fn a_shallow_base_names_the_clone_not_unrelated_history() {
    let (root, _) = two_managed_members("own-intent-shallow");
    git(&root, &["branch", "-q", "-M", "main"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha");
    let parent = root
        .parent()
        .expect("parent")
        .join("own-intent-shallow-clones");
    fs::create_dir_all(&parent).expect("parent");
    let url = format!("file://{}", root.display());
    git(
        &parent,
        &[
            "clone", "-q", "--depth", "1", "--branch", "feature", &url, "clone",
        ],
    );
    let clone = parent.join("clone");
    git(
        &clone,
        &[
            "fetch",
            "-q",
            "--depth",
            "1",
            "origin",
            "main:refs/remotes/origin/main",
        ],
    );

    let (code, _, stderr) = oakum_exit(&clone, &["check", "--strict"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("shallow clone"), "{stderr}");
    assert!(!stderr.contains("share no history"), "{stderr}");
}

/// The branch that repairs a broken pending file is the one that must pass:
/// the base's copy, unreadable, does not decide.
#[test]
fn a_branch_repairing_a_pending_file_is_covered_by_it() {
    let (root, _) = two_managed_members("own-intent-repair");
    let broken = root.join(".changeset/broken.md");
    fs::write(&broken, "---\nalpha: huge\n---\nfix\n").expect("broken");
    commit(&root, "chore: a broken bump file");
    let base = head_sha(&root);
    fs::write(&broken, "---\nalpha: patch\n---\nfix\n").expect("repair");
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha, and its bump file");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// A symlinked bump file, or a symlinked `.changeset`, pending on the base is
/// pending all the same: it covers nothing the branch changes.
#[cfg(unix)]
#[test]
fn symlinked_pending_intent_covers_nothing_the_branch_changes() {
    use std::os::unix::fs::symlink;

    let (root, _) = two_managed_members("own-intent-symlinked-file");
    fs::create_dir_all(root.join("notes")).expect("notes");
    fs::write(root.join("notes/alpha.md"), "---\nalpha: patch\n---\nfix\n").expect("target");
    symlink("../notes/alpha.md", root.join(".changeset/link.md")).expect("link");
    commit(&root, "chore: a symlinked bump file");
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha again");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );

    let (root, _) = two_managed_members("own-intent-symlinked-dir");
    fs::rename(root.join(".changeset"), root.join("changes")).expect("move");
    symlink("changes", root.join(".changeset")).expect("link");
    fs::write(
        root.join("changes/pending.md"),
        "---\nalpha: patch\n---\nfix\n",
    )
    .expect("bump");
    commit(&root, "chore: intent behind a symlinked directory");
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// again\n").expect("change");
    commit(&root, "fix: alpha again");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// Bump files pending on a base with no history in common cannot be told from
/// the branch's, which is a look that did not happen.
#[test]
fn a_base_sharing_no_history_leaves_coverage_unverified() {
    let (root, _) = two_managed_members("own-intent-orphan");
    git(&root, &["branch", "-q", "branch"]);
    git(&root, &["checkout", "-q", "--orphan", "elsewhere"]);
    commit(&root, "unrelated root");
    let unrelated = head_sha(&root);
    git(&root, &["checkout", "-q", "branch"]);

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &unrelated]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("share no history"), "{stderr}");
}

/// An edit made while resolving a merge is a change, even to a file the
/// version commit in the same range wrote.
#[test]
fn an_edit_inside_a_merge_still_counts() {
    let (root, _) = two_managed_members("version-commit-merge");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nfix alpha\n",
    )
    .expect("bump");
    commit(&root, "chore: add alpha's bump file");
    let before = head_sha(&root);
    git(&root, &["switch", "-c", "side"]);
    fs::write(root.join("beta/src/lib.rs"), "// side\n").expect("side");
    commit(&root, "fix: side");
    git(&root, &["switch", "-"]);
    let (code, _, versioned) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{versioned}");
    commit(&root, "Ship alpha");
    git(&root, &["merge", "--no-ff", "--no-commit", "side"]);
    let manifest = root.join("alpha/Cargo.toml");
    let mut alpha = fs::read_to_string(&manifest).expect("alpha");
    alpha.push_str("\n[features]\nextra = []\n");
    fs::write(&manifest, alpha).expect("alpha in the merge");
    commit(&root, "merge side");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// Deleting a bump file does not excuse a source edit made in the same commit.
#[test]
fn a_commit_consuming_a_bump_file_still_counts_its_other_changes() {
    let (root, _) = two_managed_members("consume-and-edit");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nfix alpha\n",
    )
    .expect("bump");
    commit(&root, "chore: add alpha's bump file");
    let before = head_sha(&root);
    fs::remove_file(root.join(".changeset/alpha.md")).expect("consume");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("alpha");
    commit(&root, "fix: drop the bump file, edit alpha");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &before]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A package uncovered whatever happens is one finding, not a finding and a
/// contradiction beside it. Without the guard a run says the same package is
/// covered and uncovered in the same breath.
#[test]
fn a_package_uncovered_either_way_is_named_once() {
    let (root, base) = two_managed_members("off-disk-once");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("alpha");
    fs::write(root.join("beta/src/lib.rs"), "// beta\n").expect("beta");
    commit(&root, "fix: change both");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: patch\n---\nnote\n",
    )
    .expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(
        stderr.matches("alpha (cargo)").count(),
        1,
        "alpha is uncovered either way, so it is the refusal and nothing else: {stderr}"
    );
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
    assert!(
        stderr.contains("beta (cargo): covered on disk by `.changeset/beta.md`, not by `HEAD`"),
        "{stderr}"
    );
}

/// The normal local loop — edit, `oakum add`, `check` — has both halves dirty
/// and must stay silent. The question is asked of the commits, so a change that
/// is not committed cannot be the thing an uncommitted bump file is holding up.
/// Asking it of the working tree instead would fire here, on every loop.
#[test]
fn a_change_and_its_intent_both_uncommitted_say_nothing() {
    let (root, base) = two_managed_members("off-disk-both-dirty");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("edit");
    fs::write(
        root.join(".changeset/alpha.md"),
        "---\nalpha: patch\n---\nnote\n",
    )
    .expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr, "", "the loop this runs in must be quiet: {stderr}");
}

/// Empty frontmatter covers whatever changed, so it holds a package up as
/// surely as one that names it — and the line has to name the file, not fall
/// back to describing it.
#[test]
fn an_uncommitted_empty_bump_file_is_named_as_the_cover() {
    let (root, base) = two_managed_members("off-disk-empty");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("edit");
    commit(&root, "fix: change alpha");
    fs::write(root.join(".changeset/empty.md"), "---\n---\nintentional\n").expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): covered on disk by `.changeset/empty.md`, not by `HEAD`"),
        "{stderr}"
    );

    // Two covers, because every other state here produces one and the join
    // between them is otherwise unexercised.
    fs::write(
        root.join(".changeset/also.md"),
        "---\nalpha: patch\n---\nnote\n",
    )
    .expect("second bump");
    let (code, _, both) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{both}");
    assert!(
        both.contains("covered on disk by `.changeset/also.md`, and `.changeset/empty.md`"),
        "{both}"
    );
}

/// Under `conventional-commits` both halves read commits, so there is nothing
/// for this comparison to compare and it does not run. Running it anyway names
/// `.changeset/<commit hash>`, which is not a path anyone can commit, so the
/// remedy it printed could not be carried out.
#[test]
fn commits_only_intent_has_no_disk_copy_to_report_on() {
    let root = temp_git_repo("off-disk-commits-only");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    // A stray bump file makes the intent half of the listing non-empty, which
    // is what used to send this mode down the comparison.
    fs::write(
        root.join(".changeset/stray.md"),
        "---\ndemo: patch\n---\nstray\n",
    )
    .expect("stray");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        !stderr.contains("not by `HEAD`"),
        "intent comes from commits here, so there is no disk copy to differ: {stderr}"
    );
}

/// An uncommitted bump file that holds nothing up is not reported, and it is
/// not itself a change to the package it names. The state no other test
/// reaches: intent on disk that `HEAD` has never seen, naming a package that
/// did not change, so the naive shape — report every package an uncommitted
/// file names — speaks where this must stay silent.
#[test]
fn an_untracked_bump_file_is_not_itself_a_change() {
    let (root, base) = two_managed_members("off-disk-not-a-change");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("alpha");
    commit(&root, "fix: change alpha only");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: patch\n---\nnote\n",
    )
    .expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "alpha is uncovered either way: {stderr}");
    assert!(
        !stderr.contains("beta (cargo)"),
        "beta did not change, so intent naming it holds nothing up: {stderr}"
    );
    assert!(
        !stderr.contains("changed outside `HEAD`"),
        "and a bump file is not a change to the package it names: {stderr}"
    );
}

/// The advisory half does not gate, so it says the same thing without
/// `--strict` — the mode where "I could not see half your work" matters most.
/// Nothing else holds it there: measured, gating these lines on `strict`
/// passed the whole suite.
#[test]
fn intent_head_does_not_carry_is_named_without_strict() {
    let (root, base) = two_managed_members("off-disk-advisory");
    fs::write(root.join("alpha/src/lib.rs"), "// alpha\n").expect("edit");
    commit(&root, "fix: change alpha");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\nalpha: patch\n---\nnote\n",
    )
    .expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): covered on disk by `.changeset/cover.md`, not by `HEAD`"),
        "{stderr}"
    );
}

/// `--strict` decides what a finding costs, never whether a run admits what it
/// did not read — so the advisory mode says it too, which is the mode where it
/// matters most. Measured: reporting this only under `--strict` passes every
/// other test, because every other test passes the flag.
#[test]
fn a_change_outside_head_is_named_without_strict() {
    let root = temp_git_repo("outside-head-advisory");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("demo (cargo): changed outside `HEAD`"),
        "{stderr}"
    );
}

/// Untracked is the state the bead names first and the one a contributor is
/// most often in — a new module they forgot to `git add`. Nothing else
/// observes it: the only untracked file the other tests write is a bump file,
/// which the intent filter removes before attribution, so dropping
/// `--untracked-files=all` passed every other test.
///
/// The flag is about whether an untracked path arrives at all. It is not about
/// the shape of the one that does: git's default `normal` mode collapses a new
/// directory to `newpkg/`, and `package_contains` is a prefix match that
/// absorbs the collapsed form, so attribution agrees either way (measured).
#[test]
fn an_untracked_file_inside_a_package_is_named() {
    let root = temp_git_repo("outside-head-untracked");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/extra.rs"), "// new\n").expect("untracked");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("demo (cargo): changed outside `HEAD`"),
        "{stderr}"
    );
}

/// A `git status` that cannot run is a look that did not happen, and it must
/// not take the refusal the commits already established with it. Measured
/// before the fix: the same tree exited 1 naming `demo` with a working git and
/// exited 2 naming nothing at all with a broken one — the finding vanished.
///
/// The worktree half is advisory, so what it could not read is said beside the
/// finding rather than ranked against it; the exit code is the finding's.
#[cfg(unix)]
#[test]
fn a_worktree_that_cannot_be_read_does_not_erase_the_finding() {
    let root = temp_git_repo("outside-head-unreadable");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");

    // Fails the one child under test and hands every other read to the real
    // git, so the committed half still answers and the difference is the
    // finding rather than a repository nothing can read.
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh
for a in \"$@\"; do
  if [ \"$a\" = status ]; then
    echo 'fatal: status is not available' >&2
    exit 128
  fi
done
exec {} \"$@\"
",
            ambient_tool("git").display()
        ),
    );
    let out = oakum(&root)
        .args(["check", "--strict", "--from", "v0.1.0"])
        .env("PATH", path_prefixed_by(&shim_dir))
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the finding decides the exit code, not the look that could not run: {stderr}"
    );
    assert!(
        stderr.contains("error: 1 package(s) changed with no covering intent"),
        "{stderr}"
    );
    assert!(
        stderr.contains("the working tree could not be read")
            && stderr.contains("status --porcelain -z"),
        "the unreadable worktree is reported beside the finding, not instead of it: {stderr}"
    );
}

/// The rule the line is worth having: it fires where committing would change
/// the answer, and nowhere else. A dirty tree is the normal state of the loop
/// this runs in, so a line on every one of them is a line people stop reading.
#[test]
fn a_change_outside_head_that_intent_covers_is_not_reported() {
    let root = temp_git_repo("outside-head-covered");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    fs::write(
        root.join(".changeset/demo.md"),
        "---\ndemo: patch\n---\nnote\n",
    )
    .expect("bump");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        !stderr.contains("changed outside `HEAD`"),
        "committing this changes nothing, so there is nothing to say: {stderr}"
    );
}

/// Attribution is the same longest-prefix rule the committed half uses, so a
/// path no package owns reports nothing. Without this the line fires on every
/// untracked scratch file in a workspace root and stops meaning anything.
#[test]
fn an_edit_outside_every_package_is_not_attributed_to_one() {
    let root = mixed_workspace("outside-head-unowned", "");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("README.md"), "# notes\n").expect("edit");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        !stderr.contains("changed outside `HEAD`"),
        "no package owns the workspace root: {stderr}"
    );
}

/// Two findings compete at equal severity, so `carry` picks by source order —
/// and the loser must still be reported rather than silently losing the tie.
#[test]
fn a_finding_that_loses_the_tie_is_still_reported() {
    let root = temp_git_repo("two-findings");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("demo.txt"), "changed").expect("change");
    commit(&root, "change with no covering intent");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "both are findings: {stderr}");
    assert!(
        stderr.contains("error:") && stderr.contains("also error:"),
        "the loser of a tie is still reported: {stderr}"
    );
}

/// A refusal that cannot be written must not become a panic: `eprintln!` aborts
/// on a refused write, which replaced the exit code with 101 and lost the
/// refusal. Measured on macOS; the same std behaviour is documented for Linux.
#[cfg(unix)]
#[test]
fn a_refused_stderr_write_keeps_the_exit_code() {
    let root = temp_git_repo("stderr-epipe");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    // A reader that exits at once, so every stderr write meets EPIPE.
    let mut reader = Command::new("sh")
        .args(["-c", "exit 0"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("reader");
    let pipe = reader.stdin.take().expect("pipe");
    reader.wait().expect("reader exits");
    let status = oakum(&root)
        .arg("check")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(pipe))
        .status()
        .expect("run");
    assert_eq!(
        status.code(),
        Some(1),
        "a refused write must not replace the outcome with a panic"
    );
}
