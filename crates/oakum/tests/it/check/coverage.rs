use super::*;

use support::fixture::{tm_workspace, TM_LOCK, TM_ROOT_MANIFEST};

/// `app` depends on `store` under `[<section>]`; `store` carries `store_extra`.
/// Tagged, then `store` changed in its own commit.
fn shipped_store(label: &str, section: &str, store_extra: &str, config: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"store\"]\n",
    )
    .expect("workspace");
    for (name, extra) in [
        (
            "app",
            format!("\n[{section}]\nstore = {{ path = \"../store\" }}\n"),
        ),
        ("store", store_extra.to_owned()),
    ] {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"
            ),
        )
        .expect("manifest");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    git(&root, &["tag", "app/v0.1.0"]);
    fs::write(root.join("store/src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix(store): a bug");
    root
}

/// ADR-0039: a change to a crate the config does not manage reaches the
/// managed package that ships it, and the refusal names the path.
#[test]
fn a_change_to_an_internal_crate_asks_the_shipping_package_for_coverage() {
    for (label, section, store_extra, config) in [
        ("internal-normal", "dependencies", "publish = false\n", ""),
        (
            "internal-build",
            "build-dependencies",
            "publish = false\n",
            "",
        ),
        (
            "internal-excluded",
            "dependencies",
            "",
            "exclude = [\"store\"]\n",
        ),
    ] {
        let root = shipped_store(label, section, store_extra, config);
        let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
        assert_eq!(code, Some(1), "{label}: {stderr}");
        assert!(
            stderr.contains("app (cargo): changed (via store) with no covering intent"),
            "{label}: {stderr}"
        );
    }
}

#[test]
fn a_two_hop_path_is_named_nearest_the_shipping_package_first() {
    let root = temp_git_repo("internal-two-hop");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"core\", \"store\"]\n",
    )
    .expect("workspace");
    for (name, extra) in [
        ("app", "\n[dependencies]\ncore = { path = \"../core\" }\n"),
        (
            "core",
            "publish = false\n\n[dependencies]\nstore = { path = \"../store\" }\n",
        ),
        ("store", "publish = false\n"),
    ] {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"
            ),
        )
        .expect("manifest");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    git(&root, &["tag", "app/v0.1.0"]);
    fs::write(root.join("store/src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix(store): a bug");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("app (cargo): changed (via core, store) with no covering intent"),
        "{stderr}"
    );
}

/// `alpha` and `beta` changed under one bump file (`alpha: patch`, `beta:
/// none`) and a commit that releases; `delta` changed by two commits that
/// release nothing. Returns the base to diff from.
fn every_reason(label: &str) -> (Fixture, String) {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_members(
        &root,
        &[("alpha", "0.1.0"), ("beta", "0.1.0"), ("delta", "0.1.0")],
    );
    commit(&root, "init");
    for name in ["alpha", "beta", "delta"] {
        git(&root, &["tag", &format!("{name}/v0.1.0")]);
    }
    let base = head_sha(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("alpha");
    fs::write(root.join("beta/src/lib.rs"), "// b\n").expect("beta");
    fs::write(
        root.join(".changeset/a.md"),
        "---\nalpha: patch\nbeta: none\n---\n\nalpha thing\n",
    )
    .expect("bump file");
    commit(&root, "update alpha and beta");
    fs::write(root.join("delta/src/lib.rs"), "// d\n").expect("delta");
    commit(&root, "refactor: tidy delta");
    fs::write(root.join("delta/src/lib.rs"), "// d2\n").expect("delta");
    commit(&root, "test: cover delta");
    (root, base)
}

fn coverage_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("coverage: "))
        .collect()
}

/// The coverage look's `changed` list from `check --json`.
fn changed_row(root: &Path, args: &[&str]) -> serde_json::Value {
    let (_, document, stderr) = oakum_exit(root, args);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}\n{stderr}"));
    parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "coverage")
        .expect("a coverage row")["changed"]
        .clone()
}

#[test]
fn check_says_how_each_changed_package_is_covered() {
    let (root, base) = every_reason("reasons-prose");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        coverage_lines(&stdout),
        [
            "coverage: 3 changed package(s), 3 covered, 0 uncovered",
            "coverage: covered by `.changeset/a.md` (patch): alpha (cargo)",
            "coverage: covered by `none` in `.changeset/a.md`: beta (cargo)",
            "coverage: covered by commits that release nothing (refactor, test): delta (cargo)",
        ],
        "{stdout}"
    );
}

#[test]
fn check_json_carries_each_reason_as_data() {
    let (root, base) = every_reason("reasons-json");
    let changed = changed_row(&root, &["check", "--json", "--from", &base]);
    let by_name = |name: &str| {
        changed
            .as_array()
            .expect("changed")
            .iter()
            .find(|row| row["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {changed}"))
            .clone()
    };
    assert_eq!(
        by_name("alpha")["covered_by"],
        serde_json::json!([{"kind": "bump-file", "file": ".changeset/a.md", "bump": "patch"}])
    );
    assert_eq!(
        by_name("beta")["covered_by"],
        serde_json::json!([{"kind": "none-entry", "file": ".changeset/a.md"}])
    );
    let delta = by_name("delta");
    let kinds: Vec<(&str, &str)> = delta["covered_by"]
        .as_array()
        .expect("covered_by")
        .iter()
        .map(|source| {
            (
                source["kind"].as_str().expect("kind"),
                source["type"].as_str().expect("type"),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("releaseless-commit", "refactor"),
            ("releaseless-commit", "test")
        ]
    );
    assert_eq!(delta["via"], serde_json::json!([]));
    assert_eq!(delta["released_by"], serde_json::json!([]));
    assert_eq!(delta["fix"], serde_json::Value::Null);
}

#[test]
fn check_json_says_why_a_package_is_uncovered_and_how_to_cover_it() {
    let root = shipped_store("reasons-uncovered", "dependencies", "publish = false\n", "");
    let changed = changed_row(&root, &["check", "--json", "--from", "HEAD~1"]);
    let app = &changed.as_array().expect("changed")[0];
    assert_eq!(app["name"], "app", "{changed}");
    assert_eq!(app["covered_by"], serde_json::json!([]));
    assert_eq!(
        app["via"],
        serde_json::json!([{"name": "store", "ecosystem": "cargo"}])
    );
    assert_eq!(app["released_by"][0]["type"], "fix", "{app}");
    assert_eq!(app["released_by"][0]["bump"], "patch", "{app}");
    assert_eq!(app["fix"], "oakum add --packages app:patch");
    let (_, stdout, _) = oakum_exit(&root, &["check", "--from", "HEAD~1"]);
    assert_eq!(
        coverage_lines(&stdout),
        ["coverage: 1 changed package(s), 0 covered, 1 uncovered"],
        "{stdout}"
    );
}

#[test]
fn strict_counts_every_uncovered_package() {
    let root = temp_git_repo("reasons-count");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_members(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("alpha");
    fs::write(root.join("beta/src/lib.rs"), "// b\n").expect("beta");
    commit(&root, "update both");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("error: 2 package(s) changed with no covering intent"),
        "{stderr}"
    );
}

#[test]
fn a_change_outside_head_names_the_path_it_takes() {
    let root = shipped_store(
        "reasons-unseen-via",
        "dependencies",
        "publish = false\n",
        "",
    );
    fs::write(root.join("store/src/lib.rs"), "// again\n").expect("edit");
    let (_, _, stderr) = oakum_exit(&root, &["check", "--from", "HEAD"]);
    assert!(
        stderr.contains("app (cargo): changed (via store) outside `HEAD`"),
        "{stderr}"
    );
}

/// Under intent from commits alone a bump file covers nothing, so the hint
/// names a commit rather than `oakum add`.
#[test]
fn an_uncovered_package_under_commits_alone_is_asked_for_a_commit() {
    let root = temp_git_repo("reasons-commits-uncovered");
    write_pinned_config(&root, BINARY_VERSION, "change-files = false\n");
    cargo_members(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("alpha");
    commit(&root, "fix(beta): scoped elsewhere");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "alpha (cargo): changed with no covering intent; name the package in a conventional commit"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("oakum add"), "{stderr}");
}

#[test]
fn an_empty_bump_file_is_named_as_covering_every_changed_package() {
    let root = repo_with_followup_change("reasons-empty");
    fs::write(root.join(".changeset/e.md"), "---\n---\n\nnote\n").expect("empty");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        coverage_lines(&stdout),
        [
            "coverage: 1 changed package(s), 1 covered, 0 uncovered",
            "coverage: the empty bump file `.changeset/e.md` covers every changed package",
        ],
        "{stdout}"
    );
    let changed = changed_row(&root, &["check", "--json", "--from", "HEAD~1"]);
    assert_eq!(
        changed[0]["covered_by"],
        serde_json::json!([{"kind": "empty-file", "file": ".changeset/e.md"}])
    );
}

#[test]
fn a_commit_is_named_as_covering_under_intent_from_commits() {
    let root = temp_git_repo("reasons-commits");
    write_pinned_config(&root, BINARY_VERSION, "change-files = false\n");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat: a thing");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines = coverage_lines(&stdout);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("coverage: covered by commit `")
                && line.ends_with("` (feat, minor): demo (cargo)")),
        "{stdout}"
    );
    let changed = changed_row(&root, &["check", "--json", "--from", "HEAD~1"]);
    let source = &changed[0]["covered_by"][0];
    assert_eq!(source["kind"], "commit", "{changed}");
    assert_eq!(source["type"], "feat", "{changed}");
    assert_eq!(source["bump"], "minor", "{changed}");
}

#[test]
fn status_json_carries_how_each_changed_package_is_covered() {
    let (root, base) = every_reason("reasons-status");
    let (ok, document, stderr) = oakum_output(&root, &["status", "--json", "--from", &base]);
    assert!(ok, "{stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let kinds: Vec<(&str, &str)> = parsed["changed"]
        .as_array()
        .expect("changed")
        .iter()
        .map(|row| {
            (
                row["name"].as_str().expect("name"),
                row["covered_by"][0]["kind"].as_str().expect("kind"),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("alpha", "bump-file"),
            ("beta", "none-entry"),
            ("delta", "releaseless-commit")
        ],
        "{document}"
    );
}

/// Two tagged packages `a` and `b`; returns the base to diff from.
fn two_packages(label: &str, config: &str) -> (Fixture, String) {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config);
    write_members(&root, &[("a", "0.1.0"), ("b", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "a/v0.1.0"]);
    git(&root, &["tag", "b/v0.1.0"]);
    let base = head_sha(&root);
    (root, base)
}

/// `HEAD`'s bump files are read with one git child, however many there are.
#[cfg(unix)]
#[test]
fn heads_bump_files_are_read_in_one_child() {
    use crate::support::tools::{ambient_tool, path_prefixed_by, path_shim};

    let (root, base) = two_packages("bump-files-one-child", "");
    for index in 0..5 {
        fs::write(
            root.join(format!(".changeset/note-{index}.md")),
            format!("---\na: patch\n---\n\nnote {index}\n"),
        )
        .expect("bump file");
    }
    touch(&root, "a", "change a");
    let log = sibling(&root, "bump-files-one-child.log");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\nexec {real} \"$@\"\n",
            log = log.display(),
            real = ambient_tool("git").display()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--strict", "--from", &base])
        .env("PATH", path_prefixed_by(&shim_dir))
        .output()
        .expect("check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("coverage: covered by `.changeset/note-0.md`"),
        "{stdout}"
    );
    let argv = fs::read_to_string(&log).expect("the shim logged no git child");
    let count = |exact: &str| argv.lines().filter(|line| *line == exact).count();
    assert_eq!(
        argv.lines()
            .filter(|line| line.starts_with("cat-file blob"))
            .count(),
        0,
        "{argv}"
    );
    assert_eq!(count("cat-file --batch"), 1, "{argv}");
}

/// A bump file `HEAD` cannot read is reported as the per-file read words it,
/// whether or not the read was batched: the wording reaches `--json`.
#[test]
fn an_unreadable_bump_file_at_head_is_reported_as_one_read() {
    let (root, base) = two_packages("bump-file-not-utf8", "");
    fs::write(
        root.join(".changeset/good.md"),
        "---\na: patch\n---\n\ngood\n",
    )
    .expect("bump file");
    fs::write(
        root.join(".changeset/bad.md"),
        b"---\na: patch\n---\n\n\xff\n",
    )
    .expect("bump file");
    touch(&root, "a", "change a");
    fs::write(
        root.join(".changeset/bad.md"),
        "---\na: patch\n---\n\nfixed\n",
    )
    .expect("repair");

    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--json", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stdout.contains("git cat-file blob") && stdout.contains("blob is not valid UTF-8"),
        "{stdout}"
    );
}

fn touch(root: &Path, package: &str, message: &str) {
    let file = root.join(package).join("src/lib.rs");
    let text = fs::read_to_string(&file).expect("lib.rs");
    fs::write(&file, format!("{text}// {message}\n")).expect("edit");
    commit(root, message);
}

/// ADR-0039: with conventional commits on, commits that release nothing
/// cover the packages they reach, each package on its own.
#[test]
fn commits_that_release_nothing_cover_their_packages() {
    let (root, base) = two_packages("releaseless", "");
    touch(&root, "a", "refactor(a): tidy");
    touch(&root, "b", "test(b): more cases");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");

    touch(&root, "a", "fix(a): a bug");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
    assert!(!stderr.contains("b (cargo)"), "{stderr}");
}

#[test]
fn releaseless_commits_cover_nothing_with_conventional_commits_off() {
    let (root, base) = two_packages("releaseless-off", "conventional-commits = false\n");
    touch(&root, "a", "refactor(a): tidy");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A releasing commit reaches the packages its paths touch, whatever its
/// scope names.
#[test]
fn a_fix_scoped_elsewhere_that_touches_a_package_keeps_it_uncovered() {
    let (root, base) = two_packages("releaseless-cross-scope", "");
    fs::write(root.join("a/src/lib.rs"), "// fixed from b\n").expect("edit");
    commit(&root, "fix(b): a bug");
    touch(&root, "a", "refactor(a): tidy");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A merge carries no paths of its own, so its message covers nothing.
#[test]
fn a_merge_with_a_releaseless_message_covers_nothing() {
    let (root, base) = two_packages("releaseless-evil-merge", "");
    git(&root, &["checkout", "-q", "-b", "side"]);
    touch(&root, "b", "fix(b): on the side");
    git(&root, &["checkout", "-q", "-"]);
    git(&root, &["merge", "-q", "--no-ff", "--no-commit", "side"]);
    fs::write(root.join("a/src/lib.rs"), "// edited in the merge\n").expect("edit");
    commit(&root, "chore(a): merge side");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

#[test]
fn status_reads_releaseless_commits_as_coverage() {
    let (root, base) = two_packages("releaseless-status", "");
    touch(&root, "a", "refactor(a): tidy");
    let (code, stdout, stderr) = oakum_exit(&root, &["status", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stdout.contains("Uncovered"), "{stdout}");

    touch(&root, "a", "fix(a): a bug");
    let (_, stdout, stderr) = oakum_exit(&root, &["status", "--from", &base]);
    assert!(stdout.contains("Uncovered"), "{stdout}{stderr}");
}

/// Commits are in `HEAD`, so coverage they give does not read as resting on a
/// bump file only the working tree holds.
#[test]
fn releaseless_coverage_beside_an_uncommitted_bump_file_rests_on_nothing() {
    let (root, base) = two_packages("releaseless-uncommitted", "");
    touch(&root, "a", "refactor(a): tidy");
    fs::write(
        root.join(".changeset/b.md"),
        "---\nb: patch\n---\n\nPending.\n",
    )
    .expect("bump file");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("a (cargo)"), "{stderr}");
}

#[test]
fn a_merge_from_the_base_does_not_uncover_a_releaseless_branch() {
    let (root, _) = two_packages("releaseless-merge", "");
    git(&root, &["checkout", "-q", "-b", "side"]);
    touch(&root, "a", "refactor(a): tidy");
    git(&root, &["checkout", "-q", "-"]);
    touch(&root, "b", "fix(b): on the base");
    let main_tip = head_sha(&root);
    git(&root, &["checkout", "-q", "side"]);
    git(&root, &["merge", "-q", "--no-ff", "--no-edit", &main_tip]);
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &main_tip]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// A revert of a branch commit cancels both; one of a base commit undoes a
/// release and releases.
#[test]
fn a_revert_cancels_its_target_only_inside_the_branch() {
    let (root, base) = two_packages("releaseless-revert", "");
    touch(&root, "a", "refactor(a): tidy");
    touch(&root, "a", "fix(a): a bug");
    git(&root, &["revert", "--no-edit", "HEAD"]);
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(0), "{stderr}");

    let (root, _) = two_packages("releaseless-revert-base", "");
    touch(&root, "a", "fix(a): a bug");
    let base = head_sha(&root);
    git(&root, &["revert", "--no-edit", "HEAD"]);
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
}

/// A commit worded as a revert that undoes only part of its target cancels
/// nothing: the fix still stands, so the package still needs coverage.
#[test]
fn a_partial_revert_does_not_cancel_its_target() {
    let (root, base) = two_packages("releaseless-partial-revert", "");
    let file = root.join("a/src/lib.rs");
    fs::write(&file, "// one\n// two\n").expect("edit");
    commit(&root, "fix(a): a bug");
    let fixed = head_sha(&root);
    fs::write(&file, "// one\n").expect("partial undo");
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "commit",
            "--no-verify",
            "-m",
            "Revert \"fix(a): a bug\"",
            "-m",
            &format!("This reverts commit {fixed}."),
        ],
    );
    touch(&root, "a", "refactor(a): tidy");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A binary file's patch cannot show what a revert restored, so a pair
/// touching one cancels nothing.
#[test]
fn a_revert_touching_a_binary_file_does_not_cancel_its_target() {
    let (root, _) = two_packages("releaseless-binary-revert", "");
    let file = root.join("a/data.bin");
    fs::write(&file, [0_u8, 1, 2, 3]).expect("binary");
    commit(&root, "chore(a): add data");
    let base_with_data = head_sha(&root);
    fs::write(&file, [0_u8, 9, 9, 3]).expect("fix");
    commit(&root, "fix(a): correct the data");
    let fixed = head_sha(&root);
    fs::write(&file, [0_u8, 9, 2, 3]).expect("partial undo");
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "commit",
            "--no-verify",
            "-m",
            "Revert \"fix(a): correct the data\"",
            "-m",
            &format!("This reverts commit {fixed}."),
        ],
    );
    touch(&root, "a", "refactor(a): tidy");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", &base_with_data]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("a (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

#[test]
fn a_refactor_of_an_internal_crate_covers_the_package_that_ships_it() {
    let root = shipped_store("internal-refactor", "dependencies", "publish = false\n", "");
    fs::write(root.join("store/src/lib.rs"), "// tidied\n").expect("edit");
    commit(&root, "refactor(store): tidy");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn a_dev_dependency_does_not_ask_for_coverage() {
    let root = shipped_store("internal-dev", "dev-dependencies", "publish = false\n", "");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("app (cargo)"), "{stderr}");
}

#[test]
fn a_none_entry_covers_the_shipping_package() {
    let root = shipped_store("internal-none", "dependencies", "publish = false\n", "");
    fs::write(
        root.join(".changeset/tidy.md"),
        "---\napp: none\n---\n\nTidy the store.\n",
    )
    .expect("bump file");
    commit(&root, "chore: cover app");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~2"]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// The half of the coverage question that reads commits, against the half that
/// reads the working tree. `check` lists changed packages from `from...HEAD`
/// and loads intent off disk, so a change that is staged or merely edited sits
/// in neither and the look passed over it in silence — measured on 0.3.1 in a
/// single-package repository: staged exited 0, the same content committed
/// against the same base exited 1 with `1 package(s) changed with no covering
/// intent`.
///
/// One test for four states because the defect is the difference between them.
/// Each state alone passes against a fixture that never moves: the edited run
/// and the committed run have to be the same bytes and the same base for the
/// silence to be the finding.
#[test]
fn a_change_outside_head_is_named_rather_than_passed_over() {
    let root = temp_git_repo("outside-head");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let unseen = "demo (cargo): changed outside `HEAD`";

    let (code, _, clean) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{clean}");
    assert!(
        !clean.contains(unseen),
        "a clean tree hides nothing: {clean}"
    );

    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    let (code, _, edited) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "an edit is not a finding: {edited}");
    // The whole sentence, not its opening: the remedy is the half that varies
    // with the intent mechanism, so matching the prefix alone leaves the only
    // interpolated part of the line unasserted.
    assert!(
        edited.contains(
            "demo (cargo): changed outside `HEAD`, which the coverage look does not read; \
             commit it and add a bump file"
        ),
        "{edited}"
    );

    git(&root, &["add", "-A"]);
    let (code, _, staged) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{staged}");
    assert!(
        staged.contains(unseen),
        "the index is no more visible than the worktree: {staged}"
    );

    commit(&root, "fix: touch demo");
    let (code, _, committed) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(1), "the same bytes, committed: {committed}");
    assert!(
        committed.contains("1 package(s) changed with no covering intent"),
        "{committed}"
    );
    assert!(
        !committed.contains(unseen),
        "a change in `HEAD` is seen, so it is reported as a finding and not twice: {committed}"
    );
}

/// Content a submodule holds is not something the parent repository can
/// commit, so naming its package would be advice no one can follow: `git add
/// -A` leaves the tree identical and there is nothing to bump. A moved gitlink
/// is a real change and still arrives — the two directions are what separate
/// `--ignore-submodules=dirty` from `=all`.
#[test]
fn a_submodule_is_named_when_its_commit_moves_and_not_when_it_is_merely_dirty() {
    let root = temp_git_repo("outside-head-submodule");
    let vendor = sibling(&root, "vendor-src");
    fs::create_dir_all(&vendor).expect("vendor dir");
    git(&vendor, &["init"]);
    fs::write(vendor.join("a.txt"), "one\n").expect("vendor file");
    commit(&vendor, "one");

    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    // `file://` transport is refused by default since CVE-2022-39253; the
    // fixture's git config is this suite's own, so the opt-in rides the call.
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &vendor.display().to_string(),
            "vendor",
        ],
    );
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    fs::write(root.join("vendor/untracked.txt"), "scratch\n").expect("dirty submodule");
    let (code, _, dirty) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{dirty}");
    assert!(
        !dirty.contains("changed outside `HEAD`"),
        "nothing here is committable from the parent, so there is nothing to say: {dirty}"
    );

    fs::remove_file(root.join("vendor/untracked.txt")).expect("clean the submodule");
    fs::write(vendor.join("b.txt"), "two\n").expect("second vendor file");
    commit(&vendor, "two");
    git(&root, &["-C", "vendor", "fetch", "origin"]);
    git(
        &root,
        &["-C", "vendor", "checkout", "--detach", "origin/main"],
    );

    let (code, _, moved) = oakum_exit(&root, &["check", "--strict", "--from", "v0.1.0"]);
    assert_eq!(code, Some(0), "{moved}");
    assert!(
        moved.contains("demo (cargo): changed outside `HEAD`"),
        "a moved gitlink is a real change the parent would commit: {moved}"
    );
}

/// `text` with `from` replaced, which must be there.
fn replaced(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} is not in:\n{text}");
    text.replacen(from, to, 1)
}

/// [`tm_workspace`] committed and tagged, then one commit writing `manifest`
/// and `lock` as the root files.
fn tm_branch(label: &str, config: &str, manifest: &str, lock: &str, message: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config);
    tm_workspace(&root);
    commit(&root, "init");
    git(&root, &["tag", "tm-cli/v0.1.0"]);
    fs::write(root.join("Cargo.toml"), manifest).expect("root Cargo.toml");
    fs::write(root.join("Cargo.lock"), lock).expect("Cargo.lock");
    commit(&root, message);
    root
}

fn rusqlite_bumped_lock() -> String {
    replaced(
        &replaced(
            TM_LOCK,
            "name = \"rusqlite\"\nversion = \"0.31.0\"",
            "name = \"rusqlite\"\nversion = \"0.32.0\"",
        ),
        "3333333333333333333333333333333333333333333333333333333333333333",
        "5555555555555555555555555555555555555555555555555555555555555555",
    )
}

/// oakoss/tm on 0.9.3: a branch changing only the root `Cargo.toml` and
/// `Cargo.lock` reported "no version-managed package changed" and passed.
#[test]
fn a_workspace_dependency_bump_asks_the_package_that_ships_it() {
    let manifest = replaced(TM_ROOT_MANIFEST, "version = \"0.31\"", "version = \"0.32\"");
    let root = tm_branch(
        "root-deps-bump",
        "",
        &manifest,
        &rusqlite_bumped_lock(),
        "fix(deps): bump rusqlite",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: rusqlite; \
             Cargo.toml: [workspace.dependencies] rusqlite) with no covering intent"
        ),
        "{stderr}"
    );
}

/// Renovate's `chore(deps):` releases nothing, which covers the change without
/// a bump file, as it does for a dependency bump in a member's own manifest.
#[test]
fn a_releaseless_dependency_commit_covers_a_root_change() {
    let manifest = replaced(TM_ROOT_MANIFEST, "version = \"0.31\"", "version = \"0.32\"");
    let root = tm_branch(
        "root-deps-chore",
        "",
        &manifest,
        &rusqlite_bumped_lock(),
        "chore(deps): update rust crate rusqlite to 0.32",
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stdout.contains("coverage: 1 changed package(s), 1 covered"),
        "{stdout}{stderr}"
    );
}

/// `tm-core` also ships a git `tm-store` under another key. The walk ends at the
/// member `tm-store`, whose row that git declaration must not be read as.
#[test]
fn a_member_row_is_never_the_dependency_a_lock_change_reaches_by() {
    const GIT: &str = "git+https://example.invalid/tm-store.git?branch=main";
    let with_git_store = |lock: &str| {
        let lock = replaced(
            lock,
            "name = \"tm-core\"\nversion = \"0.1.0\"\ndependencies = [\n \"tm-store\",\n]",
            &format!(
                "name = \"tm-core\"\nversion = \"0.1.0\"\ndependencies = [\n \"tm-store 0.1.0\",\n \
                 \"tm-store 0.2.0 ({GIT}#cccccccccccccccccccccccccccccccccccccccc)\",\n]"
            ),
        );
        format!(
            "{lock}\n[[package]]\nname = \"tm-store\"\nversion = \"0.2.0\"\n\
             source = \"{GIT}#cccccccccccccccccccccccccccccccccccccccc\"\n"
        )
    };
    let root = temp_git_repo("root-member-row");
    write_pinned_config(&root, BINARY_VERSION, "");
    tm_workspace(&root);
    let core_manifest = root.join("tm-core/Cargo.toml");
    let own = fs::read_to_string(&core_manifest).expect("tm-core");
    fs::write(
        &core_manifest,
        format!(
            "{own}tm-store-git = {{ package = \"tm-store\", git = \"https://example.invalid/tm-store.git\", branch = \"main\" }}\n"
        ),
    )
    .expect("tm-core");
    fs::write(root.join("Cargo.lock"), with_git_store(TM_LOCK)).expect("Cargo.lock");
    commit(&root, "init");
    fs::write(
        root.join("Cargo.lock"),
        with_git_store(&replaced(
            TM_LOCK,
            "name = \"libsqlite3-sys\"\nversion = \"0.28.0\"",
            "name = \"libsqlite3-sys\"\nversion = \"0.28.1\"",
        )),
    )
    .expect("Cargo.lock");
    commit(&root, "fix(deps): refresh the lock");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: rusqlite) with no covering intent"
        ),
        "{stderr}"
    );
}

/// A transitive row moving is named by the dependency `tm-store` declares.
#[test]
fn a_lock_only_change_asks_the_package_that_ships_it() {
    let lock = replaced(
        TM_LOCK,
        "name = \"libsqlite3-sys\"\nversion = \"0.28.0\"",
        "name = \"libsqlite3-sys\"\nversion = \"0.28.1\"",
    );
    let root = tm_branch(
        "root-lock-only",
        "",
        TM_ROOT_MANIFEST,
        &lock,
        "fix(deps): refresh the lock",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: rusqlite) with no covering intent"
        ),
        "{stderr}"
    );
}

#[test]
fn a_features_change_in_a_workspace_dependency_asks_for_coverage() {
    let manifest = replaced(
        TM_ROOT_MANIFEST,
        "features = [\"bundled\"]",
        "features = [\"bundled\", \"functions\"]",
    );
    let root = tm_branch(
        "root-deps-features",
        "",
        &manifest,
        TM_LOCK,
        "fix(deps): enable rusqlite functions",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; \
             Cargo.toml: [workspace.dependencies] rusqlite) with no covering intent"
        ),
        "{stderr}"
    );
}

#[test]
fn a_profile_change_asks_every_managed_cargo_package() {
    let manifest = format!("{TM_ROOT_MANIFEST}\n[profile.release]\nlto = true\n");
    let root = tm_branch(
        "root-profile",
        "",
        &manifest,
        TM_LOCK,
        "fix: link-time optimization",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via Cargo.toml: [profile]) with no covering intent"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("tm-store"), "{stderr}");
}

#[test]
fn check_json_names_the_root_file_a_change_came_through() {
    let lock = replaced(
        TM_LOCK,
        "name = \"libsqlite3-sys\"\nversion = \"0.28.0\"",
        "name = \"libsqlite3-sys\"\nversion = \"0.28.1\"",
    );
    let root = tm_branch(
        "root-json",
        "",
        TM_ROOT_MANIFEST,
        &lock,
        "fix(deps): refresh the lock",
    );
    let changed = changed_row(&root, &["check", "--json", "--from", "HEAD~1"]);
    let row = changed
        .as_array()
        .expect("changed")
        .iter()
        .find(|row| row["name"] == "tm-cli")
        .unwrap_or_else(|| panic!("no tm-cli row: {changed}"));
    assert_eq!(
        row["via"],
        serde_json::json!([
            { "name": "tm-core", "ecosystem": "cargo" },
            { "name": "tm-store", "ecosystem": "cargo" },
        ]),
        "{row}"
    );
    assert_eq!(
        row["through"],
        serde_json::json!([{ "file": "Cargo.lock", "name": "rusqlite" }]),
        "{row}"
    );
}

/// A lock that cannot be read is a look that did not happen (ADR-0034), not
/// a lock with nothing in it.
#[test]
fn an_unparsable_base_lock_is_unverified() {
    let root = temp_git_repo("root-garbled-lock");
    write_pinned_config(&root, BINARY_VERSION, "");
    tm_workspace(&root);
    fs::write(root.join("Cargo.lock"), "[[package]\nname = ").expect("garbled lock");
    commit(&root, "init");
    fs::write(root.join("Cargo.lock"), TM_LOCK).expect("Cargo.lock");
    commit(&root, "fix(deps): regenerate the lock");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("Cargo.lock"), "{stderr}");
}

/// `Cargo.lock` pins a git source with `#<commit>`, which the declared source
/// does not carry.
#[test]
fn a_git_dependency_moving_in_the_lock_asks_for_coverage() {
    let lock = replaced(
        TM_LOCK,
        "#aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "#bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    let root = tm_branch(
        "root-git-dep",
        "",
        TM_ROOT_MANIFEST,
        &lock,
        "fix(deps): update gitdep",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: gitdep) with no covering intent"
        ),
        "{stderr}"
    );
}

/// `tempfile` is a dev-dependency of `tm-store`, which no release ships.
#[test]
fn a_dev_dependency_bump_asks_nothing() {
    let manifest = replaced(
        TM_ROOT_MANIFEST,
        "tempfile = \"3.10\"",
        "tempfile = \"3.11\"",
    );
    let lock = replaced(
        &replaced(
            TM_LOCK,
            "name = \"tempfile\"\nversion = \"3.10.0\"",
            "name = \"tempfile\"\nversion = \"3.11.0\"",
        ),
        "name = \"fastrand\"\nversion = \"2.0.0\"",
        "name = \"fastrand\"\nversion = \"2.1.0\"",
    );
    let root = tm_branch(
        "root-dev-dep",
        "",
        &manifest,
        &lock,
        "fix(deps): bump tempfile",
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("tm-cli"), "{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );
}

#[test]
fn workspace_metadata_asks_nothing() {
    let manifest = replaced(TM_ROOT_MANIFEST, "note = \"one\"", "note = \"two\"");
    let root = tm_branch(
        "root-metadata",
        "",
        &manifest,
        TM_LOCK,
        "fix: reword the tm note",
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );
}

/// git reports the base's lock as missing, which is a side with no rows.
#[test]
fn a_lock_added_on_the_branch_counts_every_row() {
    let root = temp_git_repo("root-lock-added");
    write_pinned_config(&root, BINARY_VERSION, "");
    tm_workspace(&root);
    fs::remove_file(root.join("Cargo.lock")).expect("no lock at the base");
    commit(&root, "init");
    fs::write(root.join("Cargo.lock"), TM_LOCK).expect("Cargo.lock");
    commit(&root, "fix(deps): commit the lock");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: gitdep, rusqlite) with no covering intent"
        ),
        "{stderr}"
    );
}

/// `tm-cli` inherits a `[workspace.dependencies]` path entry for `tm-core`,
/// written `{ path = "tm-core", <base> }` and then `<head>`.
fn path_entry_branch(label: &str, base: &str, head: &str, message: &str) -> Fixture {
    let entry = |rest: &str| {
        replaced(
            TM_ROOT_MANIFEST,
            "[workspace.dependencies]\n",
            &format!("[workspace.dependencies]\ntm-core = {{ path = \"tm-core\", {rest} }}\n"),
        )
    };
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    tm_workspace(&root);
    fs::write(root.join("Cargo.toml"), entry(base)).expect("root");
    let cli = root.join("tm-cli/Cargo.toml");
    let own = fs::read_to_string(&cli).expect("tm-cli");
    fs::write(
        &cli,
        replaced(
            &own,
            "tm-core = { path = \"../tm-core\" }",
            "tm-core.workspace = true",
        ),
    )
    .expect("tm-cli");
    commit(&root, "init");
    fs::write(root.join("Cargo.toml"), entry(head)).expect("root");
    commit(&root, message);
    root
}

/// An internal crate's entry is a path, which ships through the workspace
/// graph; its `version` key, which `version` rewrites, moves nothing.
#[test]
fn an_inherited_path_entry_asks_nothing() {
    let root = path_entry_branch(
        "root-path-pin",
        "version = \"0.1.0\"",
        "version = \"0.1\"",
        "fix: loosen the tm-core pin",
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );
}

/// Features change what `tm-cli` compiles without touching the lock.
#[test]
fn a_path_entry_changed_beyond_its_version_asks_its_inheritors() {
    let root = path_entry_branch(
        "root-path-features",
        "version = \"0.1.0\"",
        "version = \"0.1.0\", default-features = false",
        "fix: drop tm-core's default features",
    );
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "tm-cli (cargo): changed (via Cargo.toml: [workspace.dependencies] tm-core) \
             with no covering intent"
        ),
        "{stderr}"
    );
}

/// What `version` writes at the root: `tm-cli`'s `Cargo.lock` row, and with
/// `version.workspace = true` the `[workspace.package]` version. A release pull
/// request carrying them asks nothing of the package it releases.
#[test]
fn a_version_commit_at_the_root_asks_nothing() {
    for (label, inherits) in [
        ("root-version-own", false),
        ("root-version-inherited", true),
    ] {
        let root = temp_git_repo(label);
        write_pinned_config(&root, BINARY_VERSION, "conventional-commits = false\n");
        tm_workspace(&root);
        let cli = root.join("tm-cli/Cargo.toml");
        let manifest_at = |version: &str| {
            if inherits {
                replaced(
                    TM_ROOT_MANIFEST,
                    "edition = \"2021\"",
                    &format!("edition = \"2021\"\nversion = \"{version}\""),
                )
            } else {
                TM_ROOT_MANIFEST.to_owned()
            }
        };
        let cli_at = |version: &str| {
            let own = fs::read_to_string(&cli).expect("tm-cli manifest");
            let line = if inherits {
                String::from("version.workspace = true")
            } else {
                format!("version = \"{version}\"")
            };
            replaced(&own, "version = \"0.1.0\"", &line)
        };
        fs::write(root.join("Cargo.toml"), manifest_at("0.1.0")).expect("root");
        let first = cli_at("0.1.0");
        fs::write(&cli, &first).expect("tm-cli");
        fs::write(
            root.join(".changeset/cli.md"),
            "---\ntm-cli: minor\n---\n\nA thing.\n",
        )
        .expect("bump file");
        commit(&root, "init");
        fs::remove_file(root.join(".changeset/cli.md")).expect("consume");
        fs::write(root.join("Cargo.toml"), manifest_at("0.2.0")).expect("root");
        if !inherits {
            fs::write(&cli, replaced(&first, "0.1.0", "0.2.0")).expect("tm-cli");
        }
        fs::write(
            root.join("tm-cli/CHANGELOG.md"),
            "# Changelog\n\n## 0.2.0\n",
        )
        .expect("changelog");
        fs::write(
            root.join("Cargo.lock"),
            replaced(
                TM_LOCK,
                "name = \"tm-cli\"\nversion = \"0.1.0\"",
                "name = \"tm-cli\"\nversion = \"0.2.0\"",
            ),
        )
        .expect("Cargo.lock");
        commit(&root, "chore(release): version packages");
        git(&root, &["tag", "tm-cli/v0.2.0"]);
        let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
        assert_eq!(code, Some(0), "{label}: {stderr}");
        assert!(
            stdout.contains("coverage: no version-managed package changed"),
            "{label}: {stdout}"
        );
    }
}
