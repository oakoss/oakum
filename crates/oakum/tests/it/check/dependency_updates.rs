//! A change that moves only dependencies needs no intent (ADR-0040).

use std::collections::BTreeMap;

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

/// `generate` from `HEAD~1` writes `lib: patch` and nothing for `app`.
fn generate_writes_lib_alone(root: &Path) {
    let (code, stdout, stderr) =
        oakum_exit(root, &["generate", "--from", "HEAD~1", "--name", "mixed"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    let written = fs::read_to_string(root.join(".changeset/mixed.md")).expect("bump file");
    assert!(written.contains("lib: patch"), "{written}");
    assert!(!written.contains("app:"), "{written}");
}

/// `fix(app)` edits `lib`'s source and moves only `app`'s requirement: `app`,
/// reached only by the update, releases nothing though the scope names it,
/// and its paths decide, so the edit in `lib` releases `lib`.
#[test]
fn a_scope_reaching_its_package_only_by_an_update_releases_what_its_paths_reach() {
    let root = two_members("deps-mixed-scope");
    fs::write(root.join("lib/src/lib.rs"), "// fixed\n").expect("lib");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(root.join("app/Cargo.toml"), moved_serde(&manifest)).expect("move");
    commit(&root, "fix(app): fix lib, update serde");
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
    generate_writes_lib_alone(&root);
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
/// `app`, which the scope then does not release: its paths decide.
#[test]
fn a_scope_reaching_its_package_only_by_a_development_edit_releases_what_its_paths_reach() {
    let root = two_members("deps-dev-mixed-scope");
    fs::write(root.join("lib/src/lib.rs"), "// fixed\n").expect("lib");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(
        root.join("app/Cargo.toml"),
        format!("{manifest}\n[dev-dependencies]\ninsta = \"1.40\"\n"),
    )
    .expect("dev");
    commit(&root, "fix(app): fix lib, add insta");
    generate_writes_lib_alone(&root);
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

/// `app` shipping the unversioned `lib`, which takes the unversioned `u`, each
/// of `lib` and `u` taking `serde`, beside the versioned `other`; `app` and
/// `other` tagged. `edit` runs on the tree before the `fix(lib)` commit that
/// moves `moved`'s serde and edits `other`; `generate` from `HEAD~1` then
/// writes what it read.
fn unversioned_lib(label: &str, moved: &str, edit: impl FnOnce(&Path)) -> String {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"lib\", \"other\", \"u\"]\n",
    )
    .expect("workspace");
    let members = [
        (
            "app",
            "src/main.rs",
            "fn main() {}\n",
            "[dependencies]\nlib = { path = \"../lib\" }\n",
        ),
        (
            "lib",
            "src/lib.rs",
            "",
            "publish = false\n\n[dependencies]\nserde = \"1.0.100\"\nu = { path = \"../u\" }\n",
        ),
        ("other", "src/lib.rs", "", ""),
        (
            "u",
            "src/lib.rs",
            "",
            "publish = false\n\n[dependencies]\nserde = \"1.0.100\"\n",
        ),
    ];
    for (name, source, body, tail) in members {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(root.join(name).join(source), body).expect("source");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{tail}"
            ),
        )
        .expect("manifest");
    }
    commit(&root, "init");
    for name in ["app", "other"] {
        git(&root, &["tag", &format!("{name}/v0.1.0")]);
    }
    let manifest = root.join(moved).join("Cargo.toml");
    let text = fs::read_to_string(&manifest).expect("manifest");
    fs::write(&manifest, moved_serde(&text)).expect("move");
    fs::write(root.join("other/src/lib.rs"), "// fixed\n").expect("other");
    edit(&root);
    commit(&root, "fix(lib): update serde, fix other");
    let (code, stdout, stderr) =
        oakum_exit(&root, &["generate", "--from", "HEAD~1", "--name", "mixed"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    fs::read_to_string(root.join(".changeset/mixed.md")).expect("bump file")
}

/// `lib`, reached only by the update, gives way to the paths: `other`
/// releases and `app` does not.
#[test]
fn an_unversioned_scope_reached_only_by_an_update_gives_way_to_the_paths() {
    let written = unversioned_lib("deps-unversioned-scope", "lib", |_| {});
    assert!(written.contains("other: patch"), "{written}");
    assert!(!written.contains("app:"), "{written}");
}

/// An edit to `u` reaches `app` through `lib`, so the scope keeps its place.
#[test]
fn an_unversioned_scope_an_edit_reaches_through_the_graph_keeps_its_place() {
    let written = unversioned_lib("deps-unversioned-through", "lib", |root| {
        fs::write(root.join("u/src/lib.rs"), "// fixed\n").expect("u");
    });
    assert!(written.contains("app: patch"), "{written}");
    assert!(!written.contains("other:"), "{written}");
}

/// An update starting below the scope, in `u`, still reaches `lib` only by
/// the update, so the scope gives way.
#[test]
fn an_unversioned_scope_an_update_passes_through_gives_way_to_the_paths() {
    let written = unversioned_lib("deps-unversioned-below", "u", |_| {});
    assert!(written.contains("other: patch"), "{written}");
    assert!(!written.contains("app:"), "{written}");
}

const RELEASE: &str = "dependency-updates = \"release\"\n";

/// Each package `status --json` plans, as `name` → (`to`, `bump`).
fn planned(root: &Path, args: &[&str]) -> BTreeMap<String, (String, String)> {
    let mut command = vec!["status", "--json"];
    command.extend_from_slice(args);
    let (code, stdout, stderr) = oakum_exit(root, &command);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("status --json");
    report["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| {
            let text = |key: &str| package[key].as_str().expect(key).to_owned();
            (text("name"), (text("to"), text("bump")))
        })
        .collect()
}

/// Under `"release"`, a dependency update plans the bin a patch and
/// `version` writes the note; the version commit consumes no bump file yet
/// passes `check`, and nothing is planned again before its tag.
#[test]
fn a_dependency_update_releases_under_release() {
    let root = cargo_app_with(
        "deps-release",
        RELEASE,
        "fix(deps): update serde",
        moved_serde,
    );
    assert_eq!(
        planned(&root, &[]),
        BTreeMap::from([(
            String::from("app"),
            (String::from("0.1.1"), String::from("patch"))
        )])
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    let changelog = fs::read_to_string(root.join("app/CHANGELOG.md")).expect("changelog");
    assert!(
        changelog.contains("### Changed\n\n- serde ^1.0.100 → ^1.0.200\n"),
        "{changelog}"
    );
    commit(&root, "chore: release app");
    let (_, stdout, stderr) = check(&root);
    assert!(
        stdout.contains("coverage: no version-managed package changed"),
        "{stdout}{stderr}"
    );
    assert!(planned(&root, &[]).is_empty());
    // Awaiting its tag, change files report nothing else of it.
    assert_eq!(
        status_json(&root)["updates_unread"],
        serde_json::json!([{"name": "app", "ecosystem": "cargo"}])
    );
}

/// The default plans nothing for the same update.
#[test]
fn a_dependency_update_plans_nothing_by_default() {
    let root = cargo_app("deps-ignore", moved_serde);
    assert!(planned(&root, &[]).is_empty());
}

/// A never-tagged package plans nothing from its range, since no tag records
/// what it last released; `--from` names the range instead.
#[test]
fn from_replaces_the_range_since_the_tag() {
    let root = cargo_app_with(
        "deps-release-from",
        RELEASE,
        "fix(deps): update serde",
        moved_serde,
    );
    git(&root, &["tag", "-d", "app/v0.1.0"]);
    assert!(planned(&root, &[]).is_empty());
    assert_eq!(
        planned(&root, &["--from", "HEAD~1"]),
        BTreeMap::from([(
            String::from("app"),
            (String::from("0.1.1"), String::from("patch"))
        )])
    );
}

/// A lock row under the managed library `lib` releases the bin above it,
/// naming the library, and not `lib`, whose users resolve its ranges.
#[test]
fn a_lock_row_under_a_library_releases_the_build_above_it() {
    let root = locked_library("deps-release-lock", true, false);
    write_pinned_config(&root, BINARY_VERSION, RELEASE);
    commit(&root, "chore: release dependency updates");
    git(&root, &["tag", "-f", "app/v0.1.0"]);
    git(&root, &["tag", "-f", "lib/v0.1.0"]);
    refresh_log(&root, "fix(deps): refresh the lock");
    assert_eq!(
        planned(&root, &[]),
        BTreeMap::from([(
            String::from("app"),
            (String::from("0.1.1"), String::from("patch"))
        )])
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    let changelog = fs::read_to_string(root.join("app/CHANGELOG.md")).expect("changelog");
    assert!(
        changelog.contains("- lib (its dependencies updated)\n"),
        "{changelog}"
    );
}

/// A library at 0.x whose requirement moves incompatibly plans the next
/// minor under zero-major; a bump file asking more wins, and a `none` one
/// lowers nothing.
#[test]
fn a_library_takes_its_declared_level_beside_bump_files() {
    let move_to_two = |root: &Path, label: &str| {
        let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
        fs::write(
            root.join("app/Cargo.toml"),
            manifest.replace("serde = \"1.0.100\"", "serde = \"2\""),
        )
        .expect("move");
        commit(root, label);
    };
    let tagged = |label: &str| {
        let root = two_members(label);
        write_pinned_config(&root, BINARY_VERSION, RELEASE);
        commit(&root, "chore: release dependency updates");
        git(&root, &["tag", "app/v0.1.0"]);
        git(&root, &["tag", "lib/v0.1.0"]);
        root
    };
    let root = tagged("deps-release-major");
    move_to_two(&root, "fix(deps): serde 2");
    assert_eq!(
        planned(&root, &[]),
        BTreeMap::from([(
            String::from("app"),
            (String::from("0.2.0"), String::from("minor"))
        )])
    );
    fs::write(
        root.join(".changeset/none.md"),
        "---\napp: none\n---\ntidy\n",
    )
    .expect("none");
    assert_eq!(planned(&root, &[])["app"].0, "0.2.0");

    let root = tagged("deps-release-file-wins");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(root.join("app/Cargo.toml"), moved_serde(&manifest)).expect("move");
    commit(&root, "fix(deps): update serde");
    fs::write(
        root.join(".changeset/feature.md"),
        "---\napp: minor\n---\na feature\n",
    )
    .expect("minor");
    assert_eq!(planned(&root, &[])["app"].0, "0.2.0");
}

/// A range whose base cannot be read, its root manifest or the member's own,
/// is `unverified`, not a plan with nothing in it.
#[test]
fn an_unreadable_base_leaves_the_plan_unverified() {
    for broken in ["Cargo.toml", "app/Cargo.toml"] {
        let root = cargo_app_with("deps-release-unread", RELEASE, "chore: note", |manifest| {
            format!("{manifest}# a note\n")
        });
        let good = fs::read_to_string(root.join(broken)).expect("manifest");
        fs::write(root.join(broken), "[package\n").expect("break");
        commit(&root, "chore: break a manifest");
        git(&root, &["tag", "-f", "app/v0.1.0"]);
        fs::write(root.join(broken), &good).expect("repair");
        let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
        fs::write(root.join("app/Cargo.toml"), moved_serde(&manifest)).expect("move");
        commit(&root, "fix(deps): repair and update serde");
        let (code, stdout, stderr) = oakum_exit(&root, &["status", "--json"]);
        assert_eq!(code, Some(2), "{broken}: {stdout}{stderr}");
        assert!(stderr.contains("unverified"), "{broken}: {stderr}");
    }
}

/// One member of [`cargo_workspace`]: its target kind, the members and the
/// external crates it takes, and whether it publishes.
struct Member<'a> {
    name: &'a str,
    kind: &'a str,
    members: &'a [&'a str],
    takes_log: bool,
    publish: bool,
}

/// A Cargo workspace of `members`, `log` locked at 0.4.34, every versioned
/// member tagged, under `"release"`.
fn cargo_workspace(label: &str, members: &[Member<'_>]) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, RELEASE);
    let names: Vec<String> = members
        .iter()
        .map(|member| format!("\"{}\"", member.name))
        .collect();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nresolver = \"2\"\nmembers = [{}]\n",
            names.join(", ")
        ),
    )
    .expect("workspace");
    let mut lock = String::from("version = 4\n\n");
    for member in members {
        let dir = root.join(member.name);
        fs::create_dir_all(dir.join("src")).expect("src");
        let (source, target) = match member.kind {
            "bin" => ("main.rs", ""),
            "cdylib" => ("lib.rs", "\n[lib]\ncrate-type = [\"cdylib\"]\n"),
            _ => ("lib.rs", ""),
        };
        let body = if source == "main.rs" {
            "fn main() {}\n"
        } else {
            ""
        };
        fs::write(dir.join("src").join(source), body).expect("source");
        let mut dependencies: Vec<String> = member
            .members
            .iter()
            .map(|on| format!("{on} = {{ path = \"../{on}\" }}"))
            .collect();
        if member.takes_log {
            dependencies.push(String::from("log = \"0.4\""));
        }
        let publish = if member.publish {
            ""
        } else {
            "publish = false\n"
        };
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{publish}{target}\n[dependencies]\n{}\n",
                member.name,
                dependencies.join("\n")
            ),
        )
        .expect("manifest");
        let mut locked: Vec<String> = member
            .members
            .iter()
            .map(|on| format!(" \"{on}\","))
            .collect();
        if member.takes_log {
            locked.push(String::from(" \"log\","));
        }
        lock.push_str("[[package]]\nname = \"");
        lock.push_str(member.name);
        lock.push_str("\"\nversion = \"0.1.0\"\ndependencies = [\n");
        lock.push_str(&locked.join("\n"));
        lock.push_str("\n]\n\n");
    }
    lock.push_str("[[package]]\n");
    lock.push_str(LOG_0_4_34);
    fs::write(root.join("Cargo.lock"), lock + "\n").expect("lock");
    commit(&root, "init");
    for member in members.iter().filter(|member| member.publish) {
        git(&root, &["tag", &format!("{}/v0.1.0", member.name)]);
    }
    root
}

fn member<'a>(name: &'a str, kind: &'a str, members: &'a [&'a str], takes_log: bool) -> Member<'a> {
    Member {
        name,
        kind,
        members,
        takes_log,
        publish: true,
    }
}

/// `version`'s note for `package` after `HEAD`'s update.
fn versioned_note(root: &Path, package: &str) -> String {
    let (code, stdout, stderr) = oakum_exit(root, &["version"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    fs::read_to_string(root.join(package).join("CHANGELOG.md")).expect("changelog")
}

/// A lock refresh on a bin's own dependency names the locked versions.
#[test]
fn a_lock_refresh_names_the_locked_versions() {
    let root = cargo_workspace("deps-release-locked", &[member("tool", "bin", &[], true)]);
    refresh_log(&root, "fix(deps): refresh the lock");
    let note = versioned_note(&root, "tool");
    assert!(note.contains("- log 0.4.34 → 0.4.35\n"), "{note}");
}

/// A requirement moved with its lock row reads once, as the requirement.
#[test]
fn a_declared_move_and_its_lock_row_read_once() {
    let root = cargo_workspace("deps-release-once", &[member("tool", "bin", &[], true)]);
    let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
    fs::write(
        root.join("tool/Cargo.toml"),
        manifest.replace("log = \"0.4\"", "log = \"0.4.35\""),
    )
    .expect("move");
    refresh_log(&root, "fix(deps): log 0.4.35");
    let note = versioned_note(&root, "tool");
    assert!(note.contains("- log ^0.4 → ^0.4.35\n"), "{note}");
    assert!(!note.contains("0.4.34 → 0.4.35"), "{note}");
}

/// A lock row a new declaration brought belongs to that declaration, which
/// is no dependency update.
#[test]
fn a_lock_row_an_added_declaration_brought_releases_nothing() {
    let root = cargo_workspace("deps-release-added", &[member("tool", "bin", &[], false)]);
    let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
    fs::write(
        root.join("tool/Cargo.toml"),
        format!("{manifest}log = \"0.4\"\n"),
    )
    .expect("add");
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("dependencies = [\n\n]", "dependencies = [\n \"log\",\n]"),
    )
    .expect("lock");
    commit(&root, "feat: log");
    assert!(planned(&root, &[]).is_empty());
}

/// A lock row under a managed library reaches a `cdylib` crate built on it,
/// which takes the build rule; the library releases nothing.
#[test]
fn a_cdylib_above_a_library_takes_its_lock_refresh() {
    let root = cargo_workspace(
        "deps-release-cdylib",
        &[
            member("lib", "lib", &[], true),
            member("native", "cdylib", &["lib"], false),
        ],
    );
    refresh_log(&root, "fix(deps): refresh the lock");
    assert_eq!(
        planned(&root, &[]),
        BTreeMap::from([(
            String::from("native"),
            (String::from("0.1.1"), String::from("patch"))
        )])
    );
}

/// Past a managed library reached through an unmanaged crate, the note names
/// the library.
#[test]
fn past_a_managed_library_the_note_names_it() {
    let store = Member {
        publish: false,
        ..member("store", "lib", &[], true)
    };
    let root = cargo_workspace(
        "deps-release-chain",
        &[
            member("app", "bin", &["core"], false),
            member("core", "lib", &["store"], false),
            store,
        ],
    );
    refresh_log(&root, "fix(deps): refresh the lock");
    let note = versioned_note(&root, "app");
    assert!(
        note.contains("- core (its dependencies updated)\n"),
        "{note}"
    );
}

/// A requirement moving incompatibly beside a source edit still releases the
/// library at its declared level, above the patch its bump file asks.
#[test]
fn a_move_beside_a_source_edit_still_adds_its_level() {
    let root = two_members("deps-release-beside");
    write_pinned_config(&root, BINARY_VERSION, RELEASE);
    commit(&root, "chore: release dependency updates");
    git(&root, &["tag", "app/v0.1.0"]);
    git(&root, &["tag", "lib/v0.1.0"]);
    fs::write(root.join("app/src/lib.rs"), "// fixed\n").expect("source");
    let manifest = fs::read_to_string(root.join("app/Cargo.toml")).expect("app");
    fs::write(
        root.join("app/Cargo.toml"),
        manifest.replace("serde = \"1.0.100\"", "serde = \"2\""),
    )
    .expect("move");
    fs::write(
        root.join(".changeset/fix.md"),
        "---\napp: patch\n---\nfix\n",
    )
    .expect("bump");
    commit(&root, "fix(app): fix and take serde 2");
    assert_eq!(planned(&root, &[])["app"].0, "0.2.0");
}

/// An owner taking two majors of one crate locks both; a refresh of one still
/// releases it, naming both.
#[test]
fn a_crate_locked_at_two_majors_still_releases_its_refresh() {
    let root = cargo_workspace("deps-release-two", &[member("tool", "bin", &[], true)]);
    let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
    fs::write(
        root.join("tool/Cargo.toml"),
        format!("{manifest}log03 = {{ package = \"log\", version = \"0.3\" }}\n"),
    )
    .expect("second major");
    let old = "name = \"log\"\nversion = \"0.3.9\"\n\
               source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    let lock =
        lock.replace(" \"log\",", " \"log 0.3.9\",\n \"log 0.4.34\",") + "\n[[package]]\n" + old;
    fs::write(root.join("Cargo.lock"), &lock).expect("lock");
    commit(&root, "chore: take log 0.3 too");
    git(&root, &["tag", "-f", "tool/v0.1.0"]);
    fs::write(
        root.join("Cargo.lock"),
        lock.replace(LOG_0_4_34, LOG_0_4_35)
            .replace(" \"log 0.4.34\",", " \"log 0.4.35\","),
    )
    .expect("refresh");
    commit(&root, "fix(deps): refresh the lock");
    let note = versioned_note(&root, "tool");
    assert!(
        note.contains("- log 0.3.9, 0.4.34 → 0.3.9, 0.4.35\n"),
        "{note}"
    );
}

/// Taking or dropping a second major of a crate the owner already takes is
/// that declaration's change, so it releases nothing on its own.
#[test]
fn a_second_major_taken_or_dropped_releases_nothing() {
    let log_0_3 = "name = \"log\"\nversion = \"0.3.9\"\n\
                   source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let second = " \"log 0.3.9\",\n \"log 0.4.34\",";
    let take = |root: &Path| {
        let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
        fs::write(
            root.join("tool/Cargo.toml"),
            format!("{manifest}log03 = {{ package = \"log\", version = \"0.3\" }}\n"),
        )
        .expect("second major");
        let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
        fs::write(
            root.join("Cargo.lock"),
            lock.replace(" \"log\",", second) + "\n[[package]]\n" + log_0_3,
        )
        .expect("lock");
    };
    let root = cargo_workspace(
        "deps-release-take-major",
        &[member("tool", "bin", &[], true)],
    );
    take(&root);
    commit(&root, "fix(deps): take log 0.3 too");
    assert!(planned(&root, &[]).is_empty(), "taken");

    let root = cargo_workspace(
        "deps-release-drop-major",
        &[member("tool", "bin", &[], true)],
    );
    let (manifest, lock) = (
        fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool"),
        fs::read_to_string(root.join("Cargo.lock")).expect("lock"),
    );
    take(&root);
    commit(&root, "chore: take log 0.3 too");
    git(&root, &["tag", "-f", "tool/v0.1.0"]);
    fs::write(root.join("tool/Cargo.toml"), manifest).expect("drop");
    fs::write(root.join("Cargo.lock"), lock).expect("lock");
    commit(&root, "fix(deps): drop log 0.3");
    assert!(planned(&root, &[]).is_empty(), "dropped");

    let root = cargo_workspace(
        "deps-release-swap-major",
        &[member("tool", "bin", &[], true)],
    );
    take(&root);
    commit(&root, "chore: take log 0.3 too");
    git(&root, &["tag", "-f", "tool/v0.1.0"]);
    let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
    fs::write(
        root.join("tool/Cargo.toml"),
        manifest.replace(
            "log03 = { package = \"log\", version = \"0.3\" }",
            "log05 = { package = \"log\", version = \"0.5\" }",
        ),
    )
    .expect("swap");
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("\"log 0.3.9\"", "\"log 0.5.0\"")
            .replace("version = \"0.3.9\"", "version = \"0.5.0\""),
    )
    .expect("swap lock");
    commit(&root, "fix(deps): log 0.5 for 0.3");
    assert!(planned(&root, &[]).is_empty(), "swapped");
}

/// A lock moving across a major its requirement allows (`>=0.3`) is an
/// update, released with both versions.
#[test]
fn a_lock_moving_across_a_major_still_releases() {
    let root = cargo_workspace("deps-release-across", &[member("tool", "bin", &[], true)]);
    let log_0_3 = "name = \"log\"\nversion = \"0.3.9\"\n\
                   source = \"registry+https://github.com/rust-lang/crates.io-index\"";
    let manifest = fs::read_to_string(root.join("tool/Cargo.toml")).expect("tool");
    fs::write(
        root.join("tool/Cargo.toml"),
        manifest.replace("log = \"0.4\"", "log = \">=0.3\""),
    )
    .expect("range");
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    fs::write(root.join("Cargo.lock"), lock.replace(LOG_0_4_34, log_0_3)).expect("0.3");
    commit(&root, "chore: allow log 0.3");
    git(&root, &["tag", "-f", "tool/v0.1.0"]);
    fs::write(root.join("Cargo.lock"), lock).expect("0.4");
    commit(&root, "fix(deps): cargo update");
    let note = versioned_note(&root, "tool");
    assert!(note.contains("- log 0.3.9 → 0.4.34\n"), "{note}");
}

fn status_json(root: &Path) -> serde_json::Value {
    let (code, stdout, stderr) = oakum_exit(root, &["status", "--json"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    serde_json::from_str(&stdout).expect("status --json")
}

/// A release made only of dependency updates says so in `status --json`; one
/// a bump file also asks for is intent.
#[test]
fn status_marks_a_release_made_only_of_dependency_updates() {
    let root = cargo_app_with(
        "deps-release-source",
        RELEASE,
        "fix(deps): update serde",
        moved_serde,
    );
    let report = status_json(&root);
    assert_eq!(
        report["packages"][0]["source"]["kind"], "dependency-update",
        "{report}"
    );
    fs::write(
        root.join(".changeset/fix.md"),
        "---\napp: patch\n---\nfix\n",
    )
    .expect("bump");
    let report = status_json(&root);
    assert_eq!(
        report["packages"][0]["source"]["kind"], "intent",
        "{report}"
    );
}

/// A package never tagged has no range, so `"release"` reads no updates for
/// it, and `status` names it; the default names nothing.
#[test]
fn status_names_a_package_whose_updates_are_not_read() {
    let root = cargo_app_with(
        "deps-release-unread-list",
        RELEASE,
        "fix(deps): update serde",
        moved_serde,
    );
    git(&root, &["tag", "-d", "app/v0.1.0"]);
    let report = status_json(&root);
    assert_eq!(
        report["updates_unread"],
        serde_json::json!([{"name": "app", "ecosystem": "cargo"}]),
        "{report}"
    );
    let (code, stdout, stderr) = oakum_exit(&root, &["status"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("Dependency updates not read: app"),
        "{stdout}"
    );

    let root = cargo_app("deps-ignore-unread-list", moved_serde);
    git(&root, &["tag", "-d", "app/v0.1.0"]);
    assert_eq!(status_json(&root)["updates_unread"], serde_json::json!([]));
}
