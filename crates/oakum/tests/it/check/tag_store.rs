use super::*;

#[test]
#[should_panic(expected = "one path segment")]
fn clone_of_rejects_nested_dest_names() {
    let origin = temp_git_repo("clone-reject");
    let _ = clone_of(&origin, "a/b");
}

/// git lists the readable tags and warns about the broken one at exit 0; a
/// listing short by a tag cannot vouch for the last release.
#[test]
fn a_broken_tag_ref_leaves_the_tag_look_unverified() {
    let root = tagged_cargo("broken-tag-ref", &["0.1.0", "0.2.0"]);
    fs::write(root.join(".git/refs/tags/v0.2.0"), "not a ref\n").expect("break the ref");
    let out = support::fixture::oakum(&root)
        .arg("check")
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ignoring broken ref refs/tags/v0.2.0"),
        "{stderr}"
    );
}

/// An unreadable `refs/tags` makes the tag listing empty and silent; only
/// `git refs verify` (git 2.47 and later) says so. Skipped where it cannot
/// show the gap: an older git, or root reading through the mode.
#[cfg(unix)]
#[test]
fn an_unreadable_tag_store_is_unverified_not_unreleased() {
    let root = tagged_cargo("unreadable-tag-store", &["0.1.0"]);
    let store = root.join(".git/refs/tags");
    let verify = std::process::Command::new("git")
        .args(["refs", "verify"])
        .current_dir(&root)
        .output()
        .expect("git");
    let (enforced, out) = {
        let _sealed = support::fixture::Sealed::set(&store, 0o000);
        let enforced = fs::read_dir(&store).is_err();
        (
            enforced,
            support::fixture::oakum(&root)
                .arg("check")
                .output()
                .expect("oakum"),
        )
    };
    if !verify.status.success() || !enforced {
        support::skip(&format!(
            "git refs verify unavailable ({}) or mode 000 not enforced ({enforced})",
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
        return;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("git refs verify"), "{stderr}");
    assert!(stderr.contains("refs/tags"), "{stderr}");
}

/// git loads a reftable stack whole, so one table it cannot open lists no
/// tags at exit 0 while `refs verify` crashes (measured, git 2.56); the store
/// is checked before any listing is trusted. Skipped where git cannot migrate
/// to reftable (before 2.46) or root reads through the mode.
#[cfg(unix)]
#[test]
fn an_unreadable_reftable_table_is_unverified_not_unreleased() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tagged_cargo("unreadable-reftable", &["0.1.0"]);
    let migrated = std::process::Command::new("git")
        .args(["refs", "migrate", "--ref-format=reftable"])
        .current_dir(&root)
        .output()
        .expect("git");
    if !migrated.status.success() {
        support::skip(&format!(
            "git cannot migrate to reftable ({})",
            String::from_utf8_lossy(&migrated.stderr).trim()
        ));
        return;
    }
    let list = fs::read_to_string(root.join(".git/reftable/tables.list")).expect("tables.list");
    let table = root
        .join(".git/reftable")
        .join(list.lines().last().expect("a table"));
    fs::set_permissions(&table, fs::Permissions::from_mode(0o000)).expect("seal the table");
    let enforced = fs::File::open(&table).is_err();
    let out = support::fixture::oakum(&root)
        .arg("check")
        .output()
        .expect("oakum");
    fs::set_permissions(&table, fs::Permissions::from_mode(0o644)).expect("unseal");
    if !enforced {
        support::skip("mode 000 not enforced");
        return;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("git reads no tags from a reftable store it cannot open"),
        "{stderr}"
    );
    assert!(
        stderr.contains(list.lines().last().expect("a table")),
        "{stderr}"
    );

    let out = support::fixture::oakum(&root)
        .arg("check")
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a readable store reads: {stderr}"
    );

    // A stack directory git may search but not list is one git still reads.
    let stack = root.join(".git/reftable");
    let out = {
        let _sealed = support::fixture::Sealed::set(&stack, 0o300);
        support::fixture::oakum(&root)
            .arg("check")
            .output()
            .expect("oakum")
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a searchable stack reads: {stderr}"
    );
}

/// A separate git dir keeps the tag store away from `.git/`, where `refs
/// verify` quotes it by absolute path; a short listing there is still
/// unverified (measured, git 2.56).
#[cfg(unix)]
#[test]
fn an_unreadable_tag_directory_in_a_separate_git_dir_is_unverified() {
    let root = tagged_cargo("separate-git-dir", &["0.1.0"]);
    git(&root, &["tag", "rel/v0.2.0"]);
    let meta = root
        .parent()
        .unwrap()
        .join(format!("separate-git-dir-meta-{}", std::process::id()));
    git(
        &root,
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", meta.display()),
        ],
    );
    let verify = std::process::Command::new("git")
        .args(["refs", "verify"])
        .current_dir(&root)
        .output()
        .expect("git");
    let rel = meta.join("refs/tags/rel");
    let (enforced, out) = {
        let _sealed = support::fixture::Sealed::set(&rel, 0o000);
        let enforced = fs::read_dir(&rel).is_err();
        (
            enforced,
            support::fixture::oakum(&root)
                .arg("check")
                .output()
                .expect("oakum"),
        )
    };
    fs::remove_dir_all(&meta).expect("remove the git dir");
    if !verify.status.success() || !enforced {
        support::skip(&format!(
            "git refs verify unavailable ({}) or mode 000 not enforced ({enforced})",
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
        return;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("refs/tags/rel/"), "{stderr}");
}

/// `refs verify` quotes `.git/refs/tags/...` from the worktree's top even
/// when run from a subdirectory, which is where `reachable-tags` runs git
/// (measured, git 2.56).
#[cfg(unix)]
#[test]
fn an_unreadable_tag_directory_seen_from_a_subdirectory_is_unverified() {
    let root = tagged_cargo("tag-store-from-subdir", &["0.1.0"]);
    git(&root, &["tag", "rel/v0.2.0"]);
    let verify = std::process::Command::new("git")
        .args(["refs", "verify"])
        .current_dir(&root)
        .output()
        .expect("git");
    let rel = root.join(".git/refs/tags/rel");
    let (enforced, out) = {
        let _sealed = support::fixture::Sealed::set(&rel, 0o000);
        let enforced = fs::read_dir(&rel).is_err();
        (
            enforced,
            support::fixture::oakum(&root.join("src"))
                .arg("reachable-tags")
                .output()
                .expect("oakum"),
        )
    };
    if !verify.status.success() || !enforced {
        support::skip(&format!(
            "git refs verify unavailable ({}) or mode 000 not enforced ({enforced})",
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
        return;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("refs/tags/rel/"), "{stderr}");
}
