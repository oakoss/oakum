//! A change that moves only dependencies needs no intent (ADR-0040).

use super::*;

/// `app`, a binary taking `serde` and the dev-only `insta`, tagged; `edit`
/// then rewrites its manifest in a `fix(deps)` commit, so the commit itself
/// asks for a release and only the dependency rule can cover it.
fn cargo_app(label: &str, edit: impl FnOnce(&str) -> String) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
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
    commit(&root, "fix(deps): update app's dependencies");
    root
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
