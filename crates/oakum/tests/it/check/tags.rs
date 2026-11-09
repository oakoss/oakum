use super::*;

#[test]
fn never_released_is_clean() {
    let root = temp_git_repo("bootstrap");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.0.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// ADR-0027 makes the two axes independent: a private package tagged but not
/// versioned is a real configuration, and `tag_managed` alone decides whether
/// its drift is oakum's business (`okm-404.31`).
#[test]
fn the_tag_axis_alone_decides_whether_a_private_package_drifts() {
    // The other axis stays on in both arms, so the management refusal — which
    // fires when NEITHER axis manages anything — cannot be what differs.
    for (private_packages, expect_drift) in [
        ("private-packages = { version = false, tag = true }\n", true),
        (
            "private-packages = { version = true, tag = false }\n",
            false,
        ),
    ] {
        let label = if expect_drift { "tag-on" } else { "tag-off" };
        let root = temp_git_repo(&format!("tag-axis-drift-{label}"));
        write_pinned_config(&root, BINARY_VERSION, private_packages);
        private_npm_package(&root, "demo", "0.2.0");
        commit(&root, "init");
        git(&root, &["tag", "v0.1.0"]);
        let (ok, _stdout, stderr) = check(&root);
        assert_eq!(
            !ok, expect_drift,
            "tag axis {private_packages:?} decides this alone: {stderr}"
        );
        if expect_drift {
            assert!(stderr.contains("0.2.0"), "{stderr}");
        } else {
            assert!(
                !stderr.contains("0.2.0"),
                "an untagged private package is not drift: {stderr}"
            );
        }
    }
}

/// The next version from a manifest below its tag would repeat or undercut a
/// released one, so the tag look fails on it and names both versions.
#[test]
fn manifest_below_tag_is_reported() {
    let root = temp_git_repo("below");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.2.0"]);

    let (ok, stdout, stderr) = check(&root);

    assert!(!ok, "expected the gap to fail the look");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("error: 1 package(s) below their highest tag"),
        "{stderr}"
    );
    assert!(!stderr.contains("bumped without a tag"), "{stderr}");
    assert!(
        stderr.contains("demo (cargo): manifest 0.1.0 is below tagged 0.2.0, so its next version would repeat or fall below a released one; set the manifest to 0.2.0"),
        "{stderr}"
    );
}

#[test]
fn manifest_above_tag_is_drift() {
    let root = temp_git_repo("above");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.2.0\n\n- next\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "expected drift");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
    assert!(
        stderr.contains("(local tags; run `git fetch --tags` if the remote is ahead)"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("never released"),
        "tagged-ahead must not also look untagged: {stderr}"
    );
}

/// Drift `version` did not write has no changelog section for its version,
/// so `check` names it as set by hand rather than as a release to cut.
#[test]
fn manifest_above_tag_with_no_section_is_named_as_set_by_hand() {
    let root = temp_git_repo("above-hand-set");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.1.0\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains(
            "demo (cargo): manifest 0.2.0 is above tagged 0.1.0 and has no changelog section, so it was set by hand; tag the version you meant (`git tag v0.2.0 <commit>`), set the manifest back to 0.1.0"
        ),
        "{stderr}"
    );
}

#[test]
fn shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-src");
    write_pinned_config(&src, BINARY_VERSION, "");
    cargo_package(&src, "demo", "0.1.0");
    commit(&src, "init");
    git(&src, &["tag", "v0.1.0"]);
    fs::write(src.join("src/lib.rs"), "// two\n").expect("second commit");
    commit(&src, "two");
    let dest = sibling(&src, "shallow");
    git(
        &src,
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ],
    );
    let (code, stdout, stderr) = oakum_exit(&dest, &["check"]);
    assert!(without_the_scope(&stdout).is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
    // The word and the code, together. `check` is the command AGENTS.md's
    // three-outcome rule names, and asserting only the word leaves ADR-0034's
    // exit code unpinned here (measured: the wiring survived mutation).
    assert_eq!(code, Some(2), "unverified is exit 2: {stderr}");
}

#[test]
fn leftover_tag_is_unverified() {
    let root = temp_git_repo("leftover");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "other-v1.0.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "leftover must not look clean");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("other-v1.0.0"), "{stderr}");
}

/// A publishable Cargo workspace whose members are tagged `<name>@<version>`.
fn tagged_workspace(root: &Path, members: &[(&str, &str)]) {
    write_members(root, members);
    commit(root, "init");
    for (name, version) in members {
        let tag = format!("{name}@{version}");
        git(root, &["tag", "-a", &tag, "-m", &tag]);
    }
}

/// oakoss/claude-plugins removed a plugin whose tags back published releases;
/// 0.8.2 read them as leftover and `check --strict` exited 2.
#[test]
fn a_removed_package_s_tags_are_history() {
    let root = temp_git_repo("retired");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("prose", "0.1.2")]);
    fs::remove_dir_all(root.join("prose")).expect("remove prose");
    write_members(&root, &[("keep", "0.1.0")]);
    commit(&root, "remove prose");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("leftover"), "{stderr}");
}

/// `prose` released 0.1.0 then 0.1.2 before it was removed.
fn retired_prose(label: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("prose", "0.1.0")]);
    write_members(&root, &[("keep", "0.1.0"), ("prose", "0.1.2")]);
    commit(&root, "release prose 0.1.2");
    git(&root, &["tag", "-a", "prose@0.1.2", "-m", "prose@0.1.2"]);
    fs::remove_dir_all(root.join("prose")).expect("remove prose");
    write_members(&root, &[("keep", "0.1.0")]);
    commit(&root, "remove prose");
    root
}

fn status_json(root: &Path) -> (Option<i32>, serde_json::Value, String) {
    let (code, stdout, stderr) = oakum_exit(root, &["status", "--json"]);
    let parsed = serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{err}: {stdout}"));
    (code, parsed, stderr)
}

/// Text output stays silent about a retired package; both documents list it
/// by its newest release.
#[test]
fn a_retired_package_is_listed_in_both_documents() {
    let root = retired_prose("retired-listed");
    let prose = serde_json::json!([{
        "name": "prose",
        "ecosystem": "cargo",
        "version": "0.1.2",
        "tag": "prose@0.1.2",
    }]);

    let (code, tags, stderr) = tags_row(&root);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(tags["retired"], prose, "{tags}");

    let (code, status, stderr) = status_json(&root);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(status["retired"], prose, "{status}");
    assert_eq!(status["retired_look"], "ran", "{status}");

    let (_, text, stderr) = oakum_exit(&root, &["check"]);
    assert!(
        !text.contains("prose") && !stderr.contains("prose"),
        "{text}{stderr}"
    );
}

#[test]
fn nothing_retired_is_an_empty_list() {
    let root = temp_git_repo("retired-none");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0")]);
    let (_, tags, _) = tags_row(&root);
    assert_eq!(tags["retired"], serde_json::json!([]), "{tags}");
    let (_, status, _) = status_json(&root);
    assert_eq!(status["retired"], serde_json::json!([]), "{status}");
    assert_eq!(status["retired_look"], "ran", "{status}");
}

/// The tag look refuses on drift it evaluated, so it still answered what is
/// retired.
#[test]
fn a_tags_look_that_refuses_after_reading_still_lists_the_retired() {
    let root = retired_prose("retired-beside-drift");
    write_members(&root, &[("keep", "0.2.0")]);
    commit(&root, "hand-set keep");
    let (code, tags, stderr) = tags_row(&root);
    assert_ne!(code, Some(0), "{stderr}");
    assert_eq!(tags["retired"][0]["tag"], "prose@0.1.2", "{tags}");
}

/// The name a tag carries is the same in either ecosystem; the manifest that
/// declared it says which. `alpha` sorts before `zulu` by name and after it
/// by ecosystem, so both documents must order them alike.
#[test]
fn retired_packages_carry_their_ecosystem_in_one_order() {
    let root = temp_git_repo("retired-npm");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("package.json"),
        "{\"name\":\"root\",\"private\":true}\n",
    )
    .expect("root");
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'pkgs/*'\n",
    )
    .expect("workspace");
    for name in ["keep", "alpha"] {
        fs::create_dir_all(root.join("pkgs").join(name)).expect("pkg");
        fs::write(
            root.join("pkgs").join(name).join("package.json"),
            format!("{{\"name\":\"{name}\",\"version\":\"1.0.0\"}}\n"),
        )
        .expect("package");
    }
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"zulu\"]\n",
    )
    .expect("cargo workspace");
    fs::create_dir_all(root.join("zulu/src")).expect("zulu src");
    fs::write(
        root.join("zulu/Cargo.toml"),
        "[package]\nname = \"zulu\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("zulu");
    fs::write(root.join("zulu/src/lib.rs"), "").expect("zulu lib");
    commit(&root, "init");
    for tag in ["keep@1.0.0", "alpha@1.0.0", "zulu@1.0.0"] {
        git(&root, &["tag", "-a", tag, "-m", tag]);
    }
    fs::remove_dir_all(root.join("pkgs/alpha")).expect("remove alpha");
    fs::remove_dir_all(root.join("zulu")).expect("remove zulu");
    fs::remove_file(root.join("Cargo.toml")).expect("remove cargo workspace");
    commit(&root, "remove alpha and zulu");

    let retired = serde_json::json!([
        { "name": "zulu", "ecosystem": "cargo", "version": "1.0.0", "tag": "zulu@1.0.0" },
        { "name": "alpha", "ecosystem": "npm", "version": "1.0.0", "tag": "alpha@1.0.0" },
    ]);
    let (_, tags, stderr) = tags_row(&root);
    assert_eq!(tags["retired"], retired, "{tags}\n{stderr}");
    let (_, status, stderr) = status_json(&root);
    assert_eq!(status["retired"], retired, "{status}\n{stderr}");
}

/// A rename with no tags of its own refuses, so neither document can say
/// what is retired: the list is absent, never an empty answer.
#[test]
fn a_rename_without_tags_lists_nothing_as_retired() {
    let root = temp_git_repo("renamed-not-listed");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("old", "0.1.2")]);
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.1.2\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");

    let (code, tags, stderr) = tags_row(&root);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(tags.get("retired").is_none(), "{tags}");

    let (code, status, stderr) = status_json(&root);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(status["retired"], serde_json::json!([]), "{status}");
    assert_eq!(status["retired_look"], "failed", "{status}");
}

#[test]
fn a_package_renamed_in_place_without_tags_names_the_retag() {
    let root = temp_git_repo("renamed");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("old", "0.1.2")]);
    let tagged = head_sha(&root);
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.1.2\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!("git tag new@0.1.2 {tagged}")),
        "{stderr}"
    );
}

/// `fresh` existed, untagged, when `prose` was released: its manifest there
/// names itself, not `prose`.
#[test]
fn an_untagged_package_elsewhere_is_not_read_as_a_rename() {
    let root = temp_git_repo("retired-beside-new");
    write_pinned_config(&root, BINARY_VERSION, "");
    write_members(
        &root,
        &[("keep", "0.1.0"), ("prose", "0.1.2"), ("fresh", "0.0.0")],
    );
    commit(&root, "init");
    git(&root, &["tag", "-a", "keep@0.1.0", "-m", "keep"]);
    git(&root, &["tag", "-a", "prose@0.1.2", "-m", "prose"]);
    fs::remove_dir_all(root.join("prose")).expect("remove prose");
    write_members(&root, &[("keep", "0.1.0"), ("fresh", "0.0.0")]);
    commit(&root, "remove prose");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("git tag fresh@"), "{stderr}");
}

#[test]
fn a_rename_names_the_newest_release_of_the_old_name() {
    let root = temp_git_repo("renamed-twice-released");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("old", "0.1.0")]);
    write_members(&root, &[("keep", "0.1.0"), ("old", "0.2.0")]);
    commit(&root, "release old 0.2.0");
    git(&root, &["tag", "-a", "old@0.2.0", "-m", "old@0.2.0"]);
    let second = head_sha(&root);
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!("git tag new@0.2.0 {second}")),
        "{stderr}"
    );
}

#[test]
fn an_unreleased_package_reusing_the_directory_is_not_a_rename() {
    let root = temp_git_repo("retired-dir-reused");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("prose", "0.1.0")]);
    fs::write(
        root.join("prose/Cargo.toml"),
        "[package]\nname = \"docs-site\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("reuse");
    commit(&root, "reuse prose's directory");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
}

/// A manifest oakum cannot read where the heir lives could hide a rename, so
/// the tags are not retired on its silence.
#[test]
fn an_unparseable_manifest_where_a_rename_could_be_is_unverified() {
    let root = temp_git_repo("renamed-unparseable");
    write_pinned_config(&root, BINARY_VERSION, "");
    write_members(&root, &[("keep", "0.1.0"), ("old", "0.1.2")]);
    commit(&root, "init");
    git(&root, &["tag", "-a", "keep@0.1.0", "-m", "keep"]);
    fs::write(root.join("old/Cargo.toml"), "[package\nname = \"old\"\n").expect("corrupt");
    commit(&root, "corrupt old");
    git(&root, &["tag", "-a", "old@0.1.2", "-m", "old"]);
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.1.2\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("`old/Cargo.toml` does not parse"),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_manifest_where_a_rename_could_be_is_unverified() {
    let root = temp_git_repo("renamed-symlink");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("old", "0.1.0")]);
    fs::create_dir_all(root.join("shared")).expect("shared");
    fs::rename(root.join("old/Cargo.toml"), root.join("shared/Cargo.toml")).expect("move");
    std::os::unix::fs::symlink("../shared/Cargo.toml", root.join("old/Cargo.toml"))
        .expect("symlink");
    commit(&root, "link old's manifest");
    git(&root, &["tag", "-d", "old@0.1.0"]);
    git(&root, &["tag", "-a", "old@0.1.0", "-m", "old"]);
    fs::remove_file(root.join("old/Cargo.toml")).expect("unlink");
    fs::remove_dir_all(root.join("shared")).expect("shared");
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("is not a regular file"), "{stderr}");
}

/// A bare tag on a retired release's commit may have released either package,
/// so it is not credited to the one that remains.
#[test]
fn a_bare_tag_beside_a_retired_one_is_unverified() {
    let root = temp_git_repo("retired-bare-twin");
    write_pinned_config(&root, BINARY_VERSION, "");
    write_members(&root, &[("keep", "0.9.0"), ("gone", "0.9.0")]);
    commit(&root, "init");
    git(&root, &["tag", "-a", "gone@0.9.0", "-m", "gone"]);
    git(&root, &["tag", "-a", "v0.9.0", "-m", "bare"]);
    fs::remove_dir_all(root.join("gone")).expect("remove gone");
    write_members(&root, &[("keep", "0.9.0")]);
    commit(&root, "remove gone");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("tag `v0.9.0` shares a commit with `gone@0.9.0`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("tag that commit under the package's name, then delete `v0.9.0`"),
        "{stderr}"
    );

    // The advice for a bare tag that released `keep`, followed as printed.
    git(
        &root,
        &["tag", "-a", "keep@0.9.0", "-m", "keep", "v0.9.0^{}"],
    );
    git(&root, &["tag", "-d", "v0.9.0"]);
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn a_package_added_back_under_a_retired_name_resumes_its_tags() {
    let root = temp_git_repo("retired-revived");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("prose", "0.1.2")]);
    fs::remove_dir_all(root.join("prose")).expect("remove prose");
    write_members(&root, &[("keep", "0.1.0")]);
    commit(&root, "remove prose");
    write_members(&root, &[("keep", "0.1.0"), ("prose", "0.1.2")]);
    commit(&root, "bring prose back");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn a_package_renamed_in_place_with_its_own_tags_passes() {
    let root = temp_git_repo("renamed-tagged");
    write_pinned_config(&root, BINARY_VERSION, "");
    tagged_workspace(&root, &[("keep", "0.1.0"), ("old", "0.1.2")]);
    fs::write(
        root.join("old/Cargo.toml"),
        "[package]\nname = \"new\"\nversion = \"0.1.3\"\nedition = \"2021\"\n",
    )
    .expect("rename");
    commit(&root, "rename old to new");
    git(&root, &["tag", "-a", "new@0.1.3", "-m", "new@0.1.3"]);
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn untagged_manifest_above_0_1_0_is_not_bootstrap() {
    let root = temp_git_repo("clobber");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "untagged 0.2.0 must not look never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("never released"), "{stderr}");
    assert!(stderr.contains("1 package"), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

/// ADR-0014 *Untagged versions*: a placeholder number owes a tag once its
/// changelog has a section, and a hand-set number below `0.1.0` owes one too.
#[test]
fn untagged_versions_owe_a_tag_unless_a_bare_placeholder() {
    let waiting = Some("has a changelog section and no tag yet");
    let cases = [
        ("0.0.0", None, None),
        ("0.1.0", None, None),
        ("0.1.0", Some("## 0.0.1\n\n- older\n"), None),
        ("0.1.0", Some("## 0.1.0\n\n- first\n"), waiting),
        ("0.1.0", Some("## demo@0.1.0\n\n- first\n"), waiting),
        ("0.0.1", Some("## 0.0.1\n\n- first\n"), waiting),
        ("0.1.0+local", Some("## 0.1.0\n\n- first\n"), waiting),
        ("0.1.0+local", Some("## 0.1.0+local\n\n- first\n"), waiting),
        (
            "0.0.5",
            None,
            Some("tag the version you meant (`git tag v0.0.5 <commit>`)"),
        ),
    ];
    for (version, changelog, owed) in cases {
        let root = temp_git_repo("untagged-rule");
        write_pinned_config(&root, BINARY_VERSION, "");
        cargo_package(&root, "demo", version);
        if let Some(body) = changelog {
            fs::write(root.join("CHANGELOG.md"), format!("# Changelog\n\n{body}"))
                .expect("changelog");
        }
        commit(&root, "init");
        let (ok, _stdout, stderr) = check(&root);
        let case = format!("{version} with {changelog:?}");
        match owed {
            None => assert!(ok, "{case}: a placeholder owes no tag: {stderr}"),
            Some(wording) => {
                assert!(!ok, "{case}: owes a tag: {stderr}");
                assert!(stderr.contains("never released"), "{case}: {stderr}");
                assert!(stderr.contains(wording), "{case}: {stderr}");
            }
        }
    }
}

/// A placeholder whose changelog cannot be read may or may not owe a tag, so
/// the look did not happen.
#[test]
fn an_unreadable_changelog_on_an_untagged_package_is_unverified() {
    let root = temp_git_repo("unreadable-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    unreadable_changelog(&root, "");
    commit(&root, "init");
    let (code, tags, stderr) = tags_row(&root);
    assert_eq!(code, Some(2), "unverified is exit 2: {stderr}");
    assert_eq!(tags["outcome"], "unverified", "{tags}");
    assert!(tags.to_string().contains("CHANGELOG.md"), "{tags}");
}

/// Above a placeholder the number alone owes a tag; the changelog only picks
/// the wording, so failing to read it must not demote the refusal.
#[test]
fn an_unreadable_changelog_does_not_hide_a_tag_the_number_owes() {
    let root = temp_git_repo("unreadable-above");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    unreadable_changelog(&root, "");
    commit(&root, "init");
    let (_code, tags, _stderr) = tags_row(&root);
    assert_eq!(tags["outcome"], "error", "{tags}");
    assert!(tags.to_string().contains("never released"), "{tags}");
}

/// One package the look cannot settle leaves the others' findings standing.
#[test]
fn an_unreadable_placeholder_changelog_keeps_its_siblings_findings() {
    let root = temp_git_repo("unreadable-sibling");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_members(&root, &[("alpha", "0.2.0"), ("beta", "0.1.0")]);
    unreadable_changelog(&root, "beta/");
    commit(&root, "init");
    let (code, tags, stderr) = tags_row(&root);
    let row = tags.to_string();
    assert!(row.contains("alpha (cargo): never released"), "{row}");
    assert!(row.contains("beta/CHANGELOG.md"), "{row}");
    assert_ne!(code, Some(0), "{stderr}");
}

/// Each member owns the changelog beside its manifest.
#[test]
fn a_members_own_changelog_decides_its_section() {
    let root = temp_git_repo("member-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_members(&root, &[("alpha", "0.1.0")]);
    fs::write(root.join("CHANGELOG.md"), "# Changelog\n").expect("root changelog");
    fs::write(
        root.join("alpha/CHANGELOG.md"),
        "# Changelog\n\n## 0.1.0\n\n- first\n",
    )
    .expect("member changelog");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("alpha (cargo): never released; 0.1.0 has a changelog section"),
        "{stderr}"
    );
}

/// A sibling's bare `## 0.3.0` does not make `demo-js` 0.3.0 one `version`
/// wrote; only its own `## demo-js@0.3.0` does.
#[test]
fn a_shared_changelog_section_vouches_only_for_the_package_it_names() {
    for (heading, pending) in [
        (
            "## 0.3.0",
            "demo-js (npm): never released, but the manifest is 0.3.0",
        ),
        (
            "## demo-js@0.3.0",
            "demo-js (npm): never released; 0.3.0 has a changelog section",
        ),
    ] {
        let root = temp_git_repo("shared-changelog");
        write_pinned_config(&root, BINARY_VERSION, "");
        cargo_package(&root, "demo", "0.3.0");
        fs::write(
            root.join("package.json"),
            r#"{"name":"demo-js","version":"0.3.0"}"#,
        )
        .expect("package.json");
        fs::write(
            root.join("CHANGELOG.md"),
            format!("# Changelog\n\n{heading}\n\n- first\n"),
        )
        .expect("changelog");
        commit(&root, "init");
        git(&root, &["tag", "demo/v0.3.0"]);
        let (ok, _stdout, stderr) = check(&root);
        assert!(!ok, "{stderr}");
        assert!(stderr.contains(pending), "{heading}: {stderr}");
        assert_eq!(
            stderr.contains("its changelog is shared, so only a `## demo-js@0.3.0` heading"),
            heading == "## 0.3.0",
            "{heading}: {stderr}"
        );
    }
}

/// A private sibling `version` does not write for leaves the changelog the
/// crate's own, so its bare heading still counts.
#[test]
fn a_changelog_beside_an_unmanaged_sibling_is_not_shared() {
    let root = temp_git_repo("unshared-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.3.0");
    private_npm_package(&root, "demo-js", "0.3.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.3.0\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("demo (cargo): never released; 0.3.0 has a changelog section"),
        "{stderr}"
    );
}

/// A commented-out or collapsed heading is no section, so the version it names
/// reads as set by hand.
#[test]
fn a_hidden_heading_does_not_vouch_for_a_version() {
    for log in [
        "# Changelog\n\n<!--\n## 0.3.0\n\n- draft\n-->\n",
        "# Changelog\n\n<details>\n<summary>old</summary>\n\n## 0.3.0\n\n</details>\n",
        "# Changelog\n\n<pre>\n## 0.3.0\n\n- draft\n</pre>\n",
        "# Changelog\n\n<SCRIPT type=\"text/x\">\n## 0.3.0\n\n</script>\n",
        "# Changelog\n\n<details>\n<!-- first --><!-- </details> -->\n\n## 0.3.0\n\n- draft\n</details>\n",
    ] {
        let root = temp_git_repo("hidden-heading");
        write_pinned_config(&root, BINARY_VERSION, "");
        cargo_package(&root, "demo", "0.3.0");
        fs::write(root.join("CHANGELOG.md"), log).expect("changelog");
        commit(&root, "init");
        let (ok, _stdout, stderr) = check(&root);
        assert!(!ok, "{stderr}");
        assert!(
            stderr.contains("demo (cargo): never released, but the manifest is 0.3.0"),
            "{log}: {stderr}"
        );
    }
}

/// A block never closed may hide any older entry, so `check` names it rather
/// than read the changelog as if the block were not there.
#[test]
fn a_changelog_left_open_is_named() {
    let root = temp_git_repo("open-html-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.1.0\n\n- first\n<details>\n<summary>older</summary>\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (code, _stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_ne!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("CHANGELOG.md leaves a `<details>` block open from line 6"),
        "{stderr}"
    );
}

/// A `<pre>`, `<script>`, `<style>` or `<textarea>` never closed runs to the
/// end of the file, hiding every older entry after it.
#[test]
fn a_changelog_left_in_an_open_pre_is_named() {
    let root = temp_git_repo("open-pre-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.1.0\n\n- first\n<textarea>\n\n## 0.0.9\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (code, _stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_ne!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("CHANGELOG.md leaves a `<textarea>` block open from line 6"),
        "{stderr}"
    );
}

/// A private sibling oakum only tags still has its version read from the
/// changelog, so it shares the file.
#[test]
fn a_tag_only_sibling_shares_the_changelog() {
    let root = temp_git_repo("tag-only-shared-changelog");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = false, tag = true }\n",
    );
    cargo_package(&root, "demo", "0.3.0");
    private_npm_package(&root, "demo-js", "0.3.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.3.0\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        !stderr.contains("0.3.0 has a changelog section"),
        "{stderr}"
    );
    assert!(
        stderr.contains("its changelog is shared, so only a `## demo@0.3.0` heading"),
        "{stderr}"
    );
}

/// The advice names the tag `release` would cut: package-prefixed when several
/// packages are tagged, and whatever `tag-format` renders when it is set.
#[test]
fn a_hand_set_version_names_the_tag_to_cut() {
    for (config, tag) in [
        ("", "beta/v0.0.5"),
        (
            "tag-format = \"{{ package }}@{{ version }}\"\n",
            "beta@0.0.5",
        ),
    ] {
        let root = temp_git_repo("hand-set-named-tag");
        write_pinned_config(&root, BINARY_VERSION, config);
        cargo_members(&root, &[("alpha", "0.0.0"), ("beta", "0.0.5")]);
        commit(&root, "init");
        let (ok, _stdout, stderr) = check(&root);
        assert!(!ok, "{stderr}");
        assert!(
            stderr.contains(&format!(
                "beta (cargo): never released, but the manifest is 0.0.5; tag the version you meant (`git tag {tag} <commit>`)"
            )),
            "{config:?}: {stderr}"
        );
    }
}

/// A `tag-format` whose tag would not read back as the package's version is
/// not offered: creating it would leave a tag `check` calls leftover.
#[test]
fn a_tag_that_would_not_read_back_is_not_offered() {
    let root = temp_git_repo("hand-set-unreadable-tag");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "tag-format = \"rel-{{ package }}-{{ version }}\"\n",
    );
    cargo_package(&root, "demo", "0.0.5");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains(
            "demo (cargo): never released, but the manifest is 0.0.5; tag the version you meant, or run"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("git tag"), "{stderr}");
}

/// A link to nowhere is a changelog oakum looked for and could not resolve,
/// not one that was never there.
#[cfg(unix)]
#[test]
fn a_dangling_changelog_symlink_on_a_placeholder_is_unverified() {
    let root = temp_git_repo("dangling-changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    std::os::unix::fs::symlink("docs/CHANGES.md", root.join("CHANGELOG.md")).expect("symlink");
    commit(&root, "init");
    let (code, tags, stderr) = tags_row(&root);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(tags["outcome"], "unverified", "{tags}");
    assert!(tags.to_string().contains("symlink"), "{tags}");
}

/// A directory where `<prefix>CHANGELOG.md` belongs: present, and never a
/// readable file.
fn unreadable_changelog(root: &Path, prefix: &str) {
    let dir = root.join(format!("{prefix}CHANGELOG.md/inner"));
    fs::create_dir_all(&dir).expect("changelog dir");
    fs::write(dir.join("keep"), "").expect("keep");
}

/// The `check --json` tags row. The changelogs look reads the same files and
/// reports them too, so only this row shows what the tags look decided.
fn tags_row(root: &Path) -> (Option<i32>, serde_json::Value, String) {
    let (code, document, stderr) = oakum_exit(root, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let row = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "tags")
        .expect("a tags row")
        .clone();
    (code, row, stderr)
}
