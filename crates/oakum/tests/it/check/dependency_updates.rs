//! A change that moves only dependencies needs no intent (ADR-0040).

use super::*;

/// `app`, a binary taking `serde` and the dev-only `insta`, tagged; `edit`
/// then rewrites its manifest in a `fix(deps)` commit, so the commit itself
/// asks for a release and only the dependency rule can cover it.
fn cargo_app(label: &str, edit: impl FnOnce(&str) -> String) -> Fixture {
    cargo_app_with(label, "", "fix(deps): update app's dependencies", edit)
}

/// [`cargo_app`] under `config`, the edit committed as `message`.
fn cargo_app_with(
    label: &str,
    config: &str,
    message: &str,
    edit: impl FnOnce(&str) -> String,
) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("app/src")).expect("app");
    fs::write(root.join("app/src/main.rs"), "fn main() {}\n").expect("main.rs");
    let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                    description = \"a tool\"\n\n[dependencies]\nserde = \"1.0.100\"\n\n\
                    [dev-dependencies]\ninsta = \"1.40\"\n";
    fs::write(root.join("app/Cargo.toml"), manifest).expect("manifest");
    commit(&root, "init");
    git(&root, &["tag", "app/v0.1.0"]);
    fs::write(root.join("app/Cargo.toml"), edit(manifest)).expect("edit");
    commit(&root, message);
    root
}

fn moved_serde(manifest: &str) -> String {
    manifest.replace("serde = \"1.0.100\"", "serde = \"1.0.200\"")
}

/// `generate` from `HEAD~1` writes nothing and says why.
fn generate_writes_nothing(root: &Path) {
    let (code, stdout, stderr) =
        oakum_exit(root, &["generate", "--from", "HEAD~1", "--name", "deps"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("no bump file written: 1 commit(s) moved only dependencies"),
        "{stdout}{stderr}"
    );
    assert!(!root.join(".changeset/deps.md").exists());
}

/// With conventional commits only, the commits are the plan, and a
/// dependency update's `fix(deps)` plans nothing.
#[test]
fn a_commits_only_dependency_update_plans_nothing() {
    let root = cargo_app_with(
        "deps-commits-only",
        "change-files = false\n",
        "fix(deps): update serde",
        moved_serde,
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["plan-intent", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stdout.contains("\"name\": \"app\""), "{stdout}");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: app (cargo)"),
        "{stdout}"
    );
}

/// A scope naming the package, or a message that is not conventional, does
/// not make a dependency update release.
#[test]
fn a_dependency_update_releases_nothing_whatever_its_message() {
    for (label, message) in [
        ("deps-scoped", "fix(app): update serde"),
        ("deps-dependabot", "Bump serde from 1.0.100 to 1.0.200"),
    ] {
        let root = cargo_app_with(label, "", message, moved_serde);
        generate_writes_nothing(&root);
        let (code, stdout, stderr) = check(&root);
        assert_eq!(code, Some(0), "{label}: {stdout}{stderr}");
    }
}

/// A `fix(deps)` commit beside a `refactor:` one leaves the branch covered.
#[test]
fn a_releasing_dependency_commit_does_not_block_a_releaseless_one() {
    let root = cargo_app("deps-beside-refactor", moved_serde);
    fs::write(root.join("app/src/main.rs"), "fn main() { }\n").expect("edit");
    commit(&root, "refactor: tidy main");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~2"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("commits that release nothing (refactor): app (cargo)"),
        "{stdout}"
    );
}

/// Members `app` and `lib`, neither taking the other, `app` taking `serde`;
/// tagged, and the base commit is `HEAD`.
fn two_members(label: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"lib\"]\n",
    )
    .expect("workspace");
    for (name, dependencies) in [("app", "serde = \"1.0.100\"\n"), ("lib", "")] {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\n{dependencies}"
            ),
        )
        .expect("manifest");
    }
    commit(&root, "init");
    root
}

/// `fix(app)` edits `lib`'s source and moves only `app`'s requirement: `app`,
/// reached only by the update, releases nothing though the scope names it,
/// and the edit in `lib` still asks for intent there.
#[test]
fn a_scope_reaching_its_package_only_by_an_update_releases_nothing_for_it() {
    let root = two_members("deps-mixed-scope");
    fs::write(root.join("lib/src/lib.rs"), "// fixed\n").expect("lib");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(root.join("app/Cargo.toml"), moved_serde(&manifest)).expect("move");
    commit(&root, "fix(app): fix lib, update serde");
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~1", "--name", "mixed"]);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(!root.join(".changeset/mixed.md").exists());
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: app (cargo)"),
        "{stdout}"
    );
    assert!(
        stderr.contains("lib (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// A range holding a dependency update beside a releasing commit that names
/// nothing still says it found nothing to write.
#[test]
fn generate_exits_0_only_when_every_commit_moved_dependencies() {
    let root = two_members("deps-mixed-range");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(root.join("app/Cargo.toml"), moved_serde(&manifest)).expect("move");
    commit(&root, "fix(deps): update serde");
    fs::write(root.join("NOTES.txt"), "notes\n").expect("notes");
    commit(&root, "fix: tweak notes");
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~2", "--name", "deps"]);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(stderr.contains("no package bumps detected"), "{stderr}");
}

/// `fix(lib)` edits `app` and leaves the root manifest unreadable: what its
/// paths touch is unknown, so a later `refactor:` cannot be said to cover
/// `app`, where it could before its paths were read.
#[test]
fn a_releasing_commit_whose_paths_went_unread_leaves_coverage_unverified() {
    let root = two_members("deps-unread-scope");
    fs::write(root.join("app/src/lib.rs"), "// changed\n").expect("app");
    fs::write(root.join("Cargo.toml"), "[workspace\n").expect("break");
    commit(&root, "fix(lib): change app");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"lib\"]\n",
    )
    .expect("repair");
    commit(&root, "chore(lib): repair the root manifest");
    fs::write(root.join("app/src/lib.rs"), "// tidied\n").expect("app");
    commit(&root, "refactor: tidy app");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~3"]);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
}

/// `fix(app)` edits `lib`'s source and adds only a development dependency to
/// `app`, which the scope then does not release.
#[test]
fn a_scope_reaching_its_package_only_by_a_development_edit_releases_nothing_for_it() {
    let root = two_members("deps-dev-mixed-scope");
    fs::write(root.join("lib/src/lib.rs"), "// fixed\n").expect("lib");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(
        root.join("app/Cargo.toml"),
        format!("{manifest}\n[dev-dependencies]\ninsta = \"1.40\"\n"),
    )
    .expect("dev");
    commit(&root, "fix(app): fix lib, add insta");
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~1", "--name", "mixed"]);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(stderr.contains("no package bumps detected"), "{stderr}");
}

/// A scoped commit changing only the lock rows of a library, which no build
/// takes in, is a dependency update.
#[test]
fn a_scoped_lock_refresh_reaching_only_a_library_writes_nothing() {
    let root = temp_git_repo("deps-lock-library");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("package.json"),
        r#"{"name":"core","version":"1.0.0","dependencies":{"is-odd":"^3.0.0"}}"#,
    )
    .expect("root");
    let lock = |integrity: &str| {
        format!(
            "lockfileVersion: '9.0'\n\nsettings:\n  autoInstallPeers: true\n  excludeLinksFromLockfile: false\n\nimporters:\n\n  .:\n    dependencies:\n      is-odd:\n        specifier: ^3.0.0\n        version: 3.0.1\n\npackages:\n\n  is-odd@3.0.1:\n    resolution: {{integrity: {integrity}}}\n\nsnapshots:\n\n  is-odd@3.0.1: {{}}\n"
        )
    };
    fs::write(root.join("pnpm-lock.yaml"), lock("sha512-odd")).expect("lock");
    commit(&root, "init");
    git(&root, &["tag", "core/v1.0.0"]);
    fs::write(root.join("pnpm-lock.yaml"), lock("sha512-odd-again")).expect("lock change");
    commit(&root, "fix(core): refresh the lock");
    generate_writes_nothing(&root);
}

/// A scoped commit whose path git cannot list as UTF-8 is attributed by its
/// scope, with a warning, instead of failing the range.
#[cfg(unix)]
#[test]
fn a_scoped_commit_whose_paths_cannot_be_listed_keeps_its_scope() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let root = two_members("deps-non-utf8");
    let blob = support::fixture::git_stdout(&root, &["rev-parse", "HEAD:lib/src/lib.rs"]);
    let mut cacheinfo = format!("100644,{blob},lib/bad").into_bytes();
    cacheinfo.push(0xff);
    let mut command = Command::new("git");
    let output = support::fixture::git_env(&mut command, &root)
        .current_dir(&root)
        .args(["update-index", "--add", "--cacheinfo"])
        .arg(OsStr::from_bytes(&cacheinfo))
        .output()
        .expect("git");
    assert!(output.status.success(), "{output:?}");
    git(
        &root,
        &["commit", "--no-verify", "-m", "fix(lib): add a file"],
    );
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~1", "--name", "bad"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(stderr.contains("paths could not be read"), "{stderr}");
    let written = fs::read_to_string(root.join(".changeset/bad.md")).expect("bump file");
    assert!(written.contains("lib: patch"), "{written}");
}

/// A development-only edit under an unknown scope is no unresolved scope.
#[test]
fn a_development_only_commit_names_no_unresolved_scope() {
    let root = cargo_app("deps-dev-scope", |manifest| {
        manifest.replace("insta = \"1.40\"", "insta = \"1.41\"")
    });
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~1", "--name", "deps"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(!stderr.contains("names no workspace package"), "{stderr}");
}

fn check(root: &Path) -> (Option<i32>, String, String) {
    oakum_exit(root, &["check", "--strict", "--from", "HEAD~1"])
}

#[test]
fn a_member_requirement_moving_is_covered_by_a_dependency_update() {
    let root = cargo_app("deps-own-move", |manifest| {
        manifest.replace(
            "serde = \"1.0.100\"",
            "serde = { version = \"1.0.200\", features = [\"derive\"] }",
        )
    });
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: app (cargo)"),
        "{stdout}"
    );
    let document = oakum_exit(&root, &["check", "--json", "--from", "HEAD~1"]).1;
    let parsed: serde_json::Value = serde_json::from_str(&document).expect("json");
    let coverage = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "coverage")
        .expect("a coverage row");
    assert_eq!(
        coverage["changed"][0]["covered_by"],
        serde_json::json!([{ "kind": "dependency-update" }]),
        "{document}"
    );
}

#[test]
fn a_move_beside_another_edit_still_asks() {
    for (label, edit) in [
        ("deps-own-description", "description = \"the tool\""),
        (
            "deps-own-added",
            "description = \"a tool\"\n[dependencies.log]\nversion = \"0.4\"",
        ),
    ] {
        let root = cargo_app(label, |manifest| {
            manifest
                .replace("serde = \"1.0.100\"", "serde = \"1.0.200\"")
                .replace("description = \"a tool\"", edit)
        });
        let (code, _, stderr) = check(&root);
        assert_eq!(code, Some(1), "{label}: {stderr}");
        assert!(
            stderr.contains("app (cargo): changed with no covering intent"),
            "{label}: {stderr}"
        );
    }
}

#[test]
fn an_added_runtime_dependency_asks() {
    let root = cargo_app("deps-own-new", |manifest| {
        manifest.replace("serde = \"1.0.100\"", "serde = \"1.0.100\"\nlog = \"0.4\"")
    });
    let (code, _, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stderr}");
}

/// okm-cgrl: nothing `app` ships changes, so nothing is asked.
#[test]
fn a_development_dependency_edit_asks_nothing() {
    let root = cargo_app("deps-own-dev", |manifest| {
        manifest.replace("insta = \"1.40\"", "insta = \"1.41\"\ntempfile = \"3\"")
    });
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );
}

/// The parsed manifest is the same on both sides: a comment or formatting
/// edit is no dependency update, so it asks as any other edit does.
#[test]
fn a_comment_only_edit_still_asks() {
    let root = cargo_app("deps-own-comment", |manifest| {
        format!("# the app\n{manifest}")
    });
    let (code, _, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stderr}");
}

/// Switching to an entry added to `[workspace.dependencies]` in the same
/// range resolves to the same declaration, so nothing moved.
#[test]
fn a_switch_to_an_inherited_entry_that_resolves_the_same_asks_nothing() {
    let root = cargo_app("deps-own-inherit", |manifest| {
        manifest.replace("serde = \"1.0.100\"", "serde.workspace = true")
    });
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n\n\
         [workspace.dependencies]\nserde = \"1.0.100\"\n",
    )
    .expect("root");
    commit(&root, "fix(deps): inherit serde");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~2"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );
}

/// The committed side is judged from `HEAD`, not from a working tree that
/// undid the commit's addition.
#[test]
fn a_committed_addition_reverted_in_the_worktree_still_asks() {
    let root = cargo_app("deps-reverted", |manifest| {
        manifest.replace("serde = \"1.0.100\"", "serde = \"1.0.100\"\nlog = \"0.4\"")
    });
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("manifest");
    fs::write(
        root.join("app/Cargo.toml"),
        manifest.replace("log = \"0.4\"\n", ""),
    )
    .expect("revert");
    let (code, _, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("app (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// Uncommitted edits are judged from the working tree: a dependency move
/// alone asks nothing, one beside a description edit asks once committed.
#[test]
fn an_uncommitted_manifest_is_judged_from_the_working_tree() {
    let root = cargo_app("deps-uncommitted", |manifest| {
        manifest.replace("insta = \"1.40\"", "insta = \"1.41\"")
    });
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("manifest");
    let moved = manifest.replace("serde = \"1.0.100\"", "serde = \"1.0.200\"");
    fs::write(root.join("app/Cargo.toml"), &moved).expect("move");
    let (code, stdout, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(!stderr.contains("outside `HEAD`"), "{stderr}");

    fs::write(
        root.join("app/Cargo.toml"),
        moved.replace("a tool", "the tool"),
    )
    .expect("describe");
    let (_, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD"]);
    assert!(
        stderr.contains("app (cargo): changed outside `HEAD`"),
        "{stderr}"
    );
}

/// `app` names the member `core` `serde`; `tool`'s registry `serde` is still
/// an ordinary dependency.
#[test]
fn a_member_alias_is_scoped_to_the_package_declaring_it() {
    let root = temp_git_repo("deps-alias-scope");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"core\", \"tool\"]\n",
    )
    .expect("workspace");
    let tool = |serde: &str| {
        format!(
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nserde = \"{serde}\"\n"
        )
    };
    for (name, manifest) in [
        (
            "app",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nserde = { package = \"core\", path = \"../core\" }\n"
                .to_owned(),
        ),
        (
            "core",
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n".to_owned(),
        ),
        ("tool", tool("1.0.100")),
    ] {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(root.join(name).join("Cargo.toml"), manifest).expect("manifest");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    fs::write(root.join("tool/Cargo.toml"), tool("1.0.200")).expect("move");
    commit(&root, "fix(deps): update serde");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: tool (cargo)"),
        "{stdout}"
    );
}

/// With a clean working tree, discovery's packages are `HEAD`'s, so a file
/// cargo needs that git does not track still answers.
#[test]
fn a_clean_worktree_needs_no_second_cargo_read() {
    let root = temp_git_repo("deps-clean-ignored");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"tool\"]\n",
    )
    .expect("workspace");
    let member = |name: &str, dependencies: &str| {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\n{dependencies}\n"
            ),
        )
        .expect("manifest");
    };
    member("app", "serde = \"1.0.100\"");
    member("tool", "");
    commit(&root, "init");
    // `ign` is on disk but not in git, so cargo cannot read `HEAD`'s tree.
    fs::write(root.join(".gitignore"), "/vendor\n").expect("ignore");
    fs::create_dir_all(root.join("vendor/ign/src")).expect("vendor");
    fs::write(
        root.join("vendor/ign/Cargo.toml"),
        "[package]\nname = \"ign\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("ign");
    fs::write(root.join("vendor/ign/src/lib.rs"), "").expect("ign lib");
    member("app", "serde = \"1.0.200\"");
    member("tool", "ign = { path = \"../vendor/ign\" }");
    commit(&root, "fix(deps): update serde, take ign");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: app (cargo)"),
        "{stdout}"
    );
    assert!(
        stderr.contains("tool (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// The root `Cargo.toml` links to `a.toml`; a commit renames `serde` there,
/// which needs intent. Retargeting the link at the unchanged `b.toml` without
/// committing leaves `HEAD` reading `a.toml`.
#[cfg(unix)]
#[test]
fn a_root_link_retargeted_in_the_worktree_leaves_head_asking() {
    let root = temp_git_repo("deps-root-link");
    write_pinned_config(&root, BINARY_VERSION, "");
    let workspace = |serde: &str| {
        format!(
            "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n\n\
             [workspace.dependencies]\nserde = {serde}\n"
        )
    };
    fs::write(root.join("a.toml"), workspace("\"1.0.100\"")).expect("a");
    fs::write(root.join("b.toml"), workspace("\"1.0.100\"")).expect("b");
    std::os::unix::fs::symlink("a.toml", root.join("Cargo.toml")).expect("link");
    fs::create_dir_all(root.join("app/src")).expect("app");
    fs::write(root.join("app/src/lib.rs"), "").expect("lib.rs");
    fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nserde.workspace = true\n",
    )
    .expect("manifest");
    commit(&root, "init");
    fs::write(
        root.join("a.toml"),
        workspace("{ version = \"1.0.100\", package = \"serde_json\" }"),
    )
    .expect("rename");
    fs::write(root.join("b.toml"), workspace("\"1.0.200\"")).expect("move");
    commit(&root, "fix(deps): update serde");
    fs::remove_file(root.join("Cargo.toml")).expect("unlink");
    std::os::unix::fs::symlink("b.toml", root.join("Cargo.toml")).expect("relink");

    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(
        !stdout.contains("covered by a dependency update"),
        "{stdout}"
    );
}

/// `app` and the bundler `web` in a pnpm workspace with no lockfile; `edit`
/// rewrites `app`'s manifest in a `fix(deps)` commit.
fn npm_app(label: &str, edit: impl FnOnce(&str) -> String) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("package.json"),
        r#"{"name":"root","private":true}"#,
    )
    .expect("root");
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("packages/app")).expect("app");
    let manifest = r#"{"name":"app","version":"1.0.0","dependencies":{"is-odd":"^3.0.0"},"devDependencies":{"vitest":"^2.0.0"}}"#;
    fs::write(root.join("packages/app/package.json"), manifest).expect("manifest");
    commit(&root, "init");
    git(&root, &["tag", "app/v1.0.0"]);
    fs::write(root.join("packages/app/package.json"), edit(manifest)).expect("edit");
    commit(&root, "fix(deps): update app's dependencies");
    root
}

#[test]
fn an_npm_range_moving_is_covered_and_a_dev_edit_asks_nothing() {
    let root = npm_app("deps-npm-move", |manifest| {
        manifest.replace("^3.0.0", "^3.1.0")
    });
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: app (npm)"),
        "{stdout}"
    );

    let root = npm_app("deps-npm-dev", |manifest| {
        manifest.replace("^2.0.0", "^3.0.0")
    });
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}"
    );

    let root = npm_app("deps-npm-other", |manifest| {
        manifest
            .replace("^3.0.0", "^3.1.0")
            .replace("1.0.0\"", "1.0.0\",\"license\":\"MIT\"")
    });
    let (code, _, stderr) = check(&root);
    assert_eq!(code, Some(1), "{stderr}");
}

const LOG_0_4_34: &str = "name = \"log\"\nversion = \"0.4.34\"\n\
    source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
    checksum = \"f9f8bd3e56ce4dfc153cf470fffbfa98c7620958b312ca5c3a4b8d5181fd13c6\"";
const LOG_0_4_35: &str = "name = \"log\"\nversion = \"0.4.35\"\n\
    source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
    checksum = \"5d15f86d12da44cd8d4ec3c73984537eb7f4cadcb9776a575a0a8a05ca11005c\"";

/// The library `lib` taking `log`, from `[workspace.dependencies]` when
/// `inherited`, and with `app` the binary taking `lib` above it; each tagged,
/// the lock holding `log` at 0.4.34.
fn locked_library(label: &str, with_app: bool, inherited: bool) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    let members = if with_app {
        "\"app\", \"lib\""
    } else {
        "\"lib\""
    };
    let shared = if inherited {
        "\n[workspace.dependencies]\nlog = \"0.4\"\n"
    } else {
        ""
    };
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{members}]\n{shared}"),
    )
    .expect("workspace");
    let log = if inherited {
        "log = { workspace = true }"
    } else {
        "log = \"0.4\""
    };
    fs::create_dir_all(root.join("lib/src")).expect("lib");
    fs::write(root.join("lib/src/lib.rs"), "").expect("lib.rs");
    fs::write(
        root.join("lib/Cargo.toml"),
        format!(
            "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             description = \"a library\"\n\n[dependencies]\n{log}\n"
        ),
    )
    .expect("lib manifest");
    let mut lock = String::from("version = 4\n\n");
    if with_app {
        fs::create_dir_all(root.join("app/src")).expect("app");
        fs::write(root.join("app/src/main.rs"), "fn main() {}\n").expect("main.rs");
        fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             description = \"a tool\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
        )
        .expect("app manifest");
        lock.push_str(
            "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"lib\",\n]\n\n",
        );
    }
    lock.push_str("[[package]]\nname = \"lib\"\nversion = \"0.1.0\"\ndependencies = [\n \"log\",\n]\n\n[[package]]\n");
    lock.push_str(LOG_0_4_34);
    fs::write(root.join("Cargo.lock"), lock + "\n").expect("lock");
    commit(&root, "init");
    let tagged: &[&str] = if with_app { &["app", "lib"] } else { &["lib"] };
    for name in tagged {
        git(&root, &["tag", &format!("{name}/v0.1.0")]);
    }
    root
}

fn refresh_log(root: &Path, message: &str) {
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    fs::write(
        root.join("Cargo.lock"),
        lock.replace(LOG_0_4_34, LOG_0_4_35),
    )
    .expect("refresh");
    commit(root, message);
}

/// A lock row under the managed library `lib` reaches the binary `app` that
/// builds it in, which no runtime edge carries past a managed package; a
/// `fix(app)` refresh then writes nothing.
#[test]
fn a_lock_row_under_a_managed_library_reaches_the_build_above_it() {
    let root = locked_library("deps-lock-above-lib", true, false);
    refresh_log(&root, "fix(app): refresh the lock");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    let want = "coverage: covered by a dependency update: app (cargo) (via Cargo.lock: log), \
                lib (cargo) (via Cargo.lock: log)";
    assert!(stdout.lines().any(|got| got == want), "{stdout}");
    generate_writes_nothing(&root);
}

/// With no build above it, the library alone is reached.
#[test]
fn a_lock_row_under_a_library_alone_reaches_only_the_library() {
    let root = locked_library("deps-lock-lib-alone", false, false);
    refresh_log(&root, "fix(deps): refresh the lock");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "coverage: covered by a dependency update: lib (cargo) (via Cargo.lock: log)"
        ),
        "{stdout}"
    );
}

/// A `[workspace.dependencies]` entry `lib` takes reaches `lib` alone: only a
/// lock row is carried to the build above.
#[test]
fn a_shared_entry_under_a_managed_library_does_not_reach_the_build_above_it() {
    let root = locked_library("deps-shared-above-lib", true, true);
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("root");
    fs::write(
        root.join("Cargo.toml"),
        manifest.replace("log = \"0.4\"", "log = \"0.4.20\""),
    )
    .expect("move");
    commit(&root, "fix(deps): raise log");
    let (code, stdout, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("coverage: covered by a dependency update: lib (cargo)"),
        "{stdout}"
    );
    assert!(!stdout.contains("app (cargo)"), "{stdout}");
}
