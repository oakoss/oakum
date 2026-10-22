//! A range rewrite that leaves `pnpm-lock.yaml` behind fails a frozen install
//! (`ERR_PNPM_OUTDATED_LOCKFILE`). `version` must leave a lockfile the next
//! `pnpm install --frozen-lockfile` accepts.

use crate::support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::fixture::{commit, git, git_repo, oakum, pin_oakum, Fixture};

/// Scoped so no member name can resolve from the registry.
const MAJOR_A: &str = "---\n\"@oakum-fixture/a\": major\n---\n\nbreak a\n";

fn pnpm(dir: &Path, args: &[&str]) -> Output {
    support::command_on_path("pnpm")
        .args(args)
        .arg(format!(
            "--config.store-dir={}",
            dir.join(".pnpm-store").display()
        ))
        .current_dir(dir)
        .env("CI", "true")
        .output()
        .unwrap_or_else(|err| panic!("pnpm {args:?}: {err}"))
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `(directory under the workspace, name, the manifest's dependency tables)`.
type Member<'a> = (&'a str, &'a str, &'a str);

/// `b` takes `a` through `workspace:^1.0.0`, `c` through the default catalog,
/// which links the member only under `linkWorkspacePackages`.
const MEMBERS: [Member<'static>; 3] = [
    ("packages/a", "a", ""),
    (
        "packages/b",
        "b",
        ",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  }",
    ),
    (
        "packages/c",
        "c",
        ",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"catalog:\"\n  }",
    ),
];

/// A seeded, tagged pnpm workspace in `base` under a fresh repository. Returns
/// the repository and the workspace directory, where pnpm and oakum run.
/// `root_fields` is appended to the private root manifest.
fn workspace(
    label: &str,
    base: &str,
    globs: &[&str],
    members: &[Member<'_>],
    root_fields: &str,
) -> (Fixture, PathBuf) {
    let root = git_repo("pnpm-lock", label);
    let dir = root.join(base);
    fs::create_dir_all(&dir).expect("workspace dir");
    fs::write(root.join(".gitignore"), "node_modules/\n.pnpm-store/\n").expect("gitignore");
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/_config.toml"), "").expect("config");
    pin_oakum(&root);
    let packages = globs.join("'\n  - '");
    fs::write(
        dir.join("pnpm-workspace.yaml"),
        format!(
            "packages:\n  - '{packages}'\nlinkWorkspacePackages: true\ncatalog:\n  '@oakum-fixture/a': ^1.0.0\n  alias-a: npm:@oakum-fixture/a@^1.0.0\n"
        ),
    )
    .expect("workspace yaml");
    fs::write(
        dir.join("package.json"),
        format!("{{\n  \"name\": \"root\",\n  \"private\": true{root_fields}\n}}\n"),
    )
    .expect("root manifest");
    for (path, name, deps) in members {
        let member = dir.join(path);
        fs::create_dir_all(&member).expect("member dir");
        fs::write(
            member.join("package.json"),
            format!(
                "{{\n  \"name\": \"@oakum-fixture/{name}\",\n  \"version\": \"1.0.0\"{deps}\n}}\n"
            ),
        )
        .expect("member manifest");
    }
    let install = pnpm(&dir, &["install", "--offline", "--no-frozen-lockfile"]);
    assert!(install.status.success(), "seed install: {}", text(&install));
    assert!(dir.join("pnpm-lock.yaml").is_file());
    commit(&root, "seed");
    for (_, name, _) in members {
        let tag = format!("@oakum-fixture/{name}/v1.0.0");
        git(&root, &["tag", "-a", &tag, "-m", &tag]);
    }
    (root, dir)
}

/// Runs `oakum version` on a major bump of `a`, then asserts a frozen install
/// accepts the lockfile and that pnpm's own regeneration matches it byte for
/// byte.
fn assert_version_leaves_a_frozen_lock(root: &Path, dir: &Path) {
    fs::write(root.join(".changeset/one.md"), MAJOR_A).expect("bump file");
    let run = oakum(dir).arg("version").output().expect("oakum version");
    assert!(run.status.success(), "oakum version: {}", text(&run));
    assert!(
        !String::from_utf8_lossy(&run.stderr).contains("pnpm install"),
        "an entry was left: {}",
        text(&run)
    );

    let lock = fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock");
    let frozen = pnpm(dir, &["install", "--offline", "--frozen-lockfile"]);
    assert!(
        frozen.status.success(),
        "frozen install after version: {}\nlock:\n{lock}",
        text(&frozen)
    );
    let regenerated = pnpm(dir, &["install", "--offline", "--no-frozen-lockfile"]);
    assert!(regenerated.status.success(), "{}", text(&regenerated));
    assert_eq!(
        fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock"),
        lock
    );
}

#[test]
fn a_major_bump_leaves_a_lockfile_a_frozen_install_accepts() {
    let (root, dir) = workspace("major", "", &["packages/*"], &MEMBERS, "");
    assert_version_leaves_a_frozen_lock(&root, &dir);
    let b = fs::read_to_string(dir.join("packages/b/package.json")).expect("b");
    assert!(
        b.contains("\"@oakum-fixture/a\": \"workspace:^2.0.0\""),
        "{b}"
    );
}

/// Each member is a shape pnpm records differently from its manifest: `e`
/// outside the lock dir, `d` an auto-installed peer, `f` and `i` one key in
/// two sections, `g` a catalog alias, `h` an unmoved recorded dev range.
/// `packageManager` puts an environment document ahead of the lockfile.
#[test]
fn a_nested_workspace_with_peers_and_outside_members_stays_frozen() {
    let mut members = MEMBERS.to_vec();
    members.extend([
        (
            "../tools/e",
            "e",
            ",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  }",
        ),
        (
            "packages/d",
            "d",
            ",\n  \"peerDependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  }",
        ),
        (
            "packages/f",
            "f",
            ",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  },\n  \"devDependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  }",
        ),
        (
            "packages/g",
            "g",
            ",\n  \"dependencies\": {\n    \"alias-a\": \"catalog:\"\n  }",
        ),
        (
            "packages/h",
            "h",
            ",\n  \"peerDependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  },\n  \"devDependencies\": {\n    \"@oakum-fixture/a\": \"workspace:*\"\n  }",
        ),
        (
            "packages/i",
            "i",
            ",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:^1.0.0\"\n  },\n  \"optionalDependencies\": {\n    \"@oakum-fixture/a\": \"workspace:~1.0.0\"\n  }",
        ),
    ]);
    let version = pnpm(Path::new("."), &["--version"]);
    let running = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    let (root, dir) = workspace(
        "nested",
        "js",
        &["packages/*", "../tools/*"],
        &members,
        &format!(",\n  \"packageManager\": \"pnpm@{running}\""),
    );
    let seeded = fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock");
    assert!(
        seeded.starts_with("---\n") && seeded.contains("packageManagerDependencies"),
        "{seeded}"
    );
    assert!(seeded.contains("  ../tools/e:\n"), "{seeded}");
    assert!(
        seeded.contains("  packages/i:\n    optionalDependencies:\n"),
        "{seeded}"
    );
    assert!(
        seeded.contains("  packages/d:\n    dependencies:\n"),
        "{seeded}"
    );
    assert_version_leaves_a_frozen_lock(&root, &dir);
}

#[test]
fn an_entry_it_cannot_retarget_is_named_and_left() {
    let (root, dir) = workspace("drifted", "", &["packages/*"], &MEMBERS, "");
    let lock = fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock");
    let drifted = lock.replacen(
        "specifier: workspace:^1.0.0\n        version: link",
        "specifier: workspace:^1.1.0\n        version: link",
        1,
    );
    assert_ne!(drifted, lock, "fixture lock changed shape:\n{lock}");
    fs::write(dir.join("pnpm-lock.yaml"), &drifted).expect("drift");
    fs::write(root.join(".changeset/one.md"), MAJOR_A).expect("bump file");

    let run = oakum(&dir).arg("version").output().expect("oakum version");
    assert!(run.status.success(), "oakum version: {}", text(&run));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("importers/packages/b/dependencies/@oakum-fixture/a")
            && stderr.contains("run `pnpm install`"),
        "{stderr}"
    );
    let after = fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock");
    assert!(after.contains("specifier: workspace:^1.1.0"), "{after}");
    // The entry it could rewrite still moved.
    assert!(
        after.contains("specifier: ^2.0.0\n      version: ^2.0.0"),
        "{after}"
    );
}
