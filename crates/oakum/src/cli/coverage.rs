use std::collections::{BTreeMap, BTreeSet};

use oakum::commits::packages_for_paths;
use oakum::plan::{BumpFile, Package, PackageId, Workspace};
use oakum::state::Coverage;

use super::generate::resolve_from_ref;
use super::git::{Git, Op};
use super::CliError;

/// What the config says about a changed package. `Excluded` is a decision
/// someone wrote in `include`/`exclude`; `Unmanaged` is the absence of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Standing {
    Excluded,
    Unmanaged,
    Managed,
}

/// Changed packages the selection keeps, split by whether the config manages
/// them. `unmanaged` never reaches coverage: a package the config cannot plan
/// cannot be covered by intent either, so the two are separate reports. Both
/// are sorted and disjoint.
pub(super) fn changed_by_standing(
    git: &Git,
    workspace: &Workspace,
    files: &[BumpFile],
    from: Option<&str>,
    standing: impl Fn(&Package) -> Standing,
) -> Result<Coverage, CliError> {
    Ok(answer(
        &changed_paths(git, from)?,
        workspace,
        files,
        &classify(workspace, standing),
    ))
}

/// Managed packages whose change lives outside `HEAD` and which nothing would
/// cover once committed.
///
/// A newtype because the alternative — a bare `Vec<PackageId>` beside
/// [`Coverage::uncovered`] — is the same shape as the list it must never be
/// confused with. Swapping the two compiles cleanly and inverts the report: a
/// dirty worktree starts failing `--strict` while a real uncovered commit
/// passes.
#[derive(Debug, Default)]
pub(super) struct Unseen(Vec<PackageId>);

impl Unseen {
    /// The report, built here so the ids never become a bare slice beside
    /// `Coverage::uncovered`. That slice is the one place the two could be
    /// swapped, so a newtype unwrapped before reaching it guards the plumbing
    /// and abandons the use site.
    pub(super) fn lines(&self, hint: &str) -> Vec<String> {
        self.0
            .iter()
            .map(|id| {
                format!(
                    "{id}: changed outside `HEAD`, which the coverage look does not read; \
                     commit it and {hint}"
                )
            })
            .collect()
    }
}

/// What the coverage look established, in the two tenses it has to answer in.
pub(super) struct Uncovered {
    /// The answer for `HEAD`, which is the one a pull request is judged on.
    pub(super) committed: Coverage,
    /// What the same question would add once the working tree is committed.
    ///
    /// `Err` is a look that could not run, never a clean answer, and it is
    /// held here rather than raised so that it cannot take [`Self::committed`]
    /// with it: a `git status` that fails says nothing about the refusal the
    /// commits already established. The caller reports it without gating,
    /// because this half does not gate when it answers either.
    pub(super) outside_head: Result<Unseen, CliError>,
}

/// The coverage answer, beside the packages it would also refuse once
/// everything outside `HEAD` is committed.
///
/// Under `change-files` the two halves read different worlds — changed
/// packages from commits, intent from the working tree — so a change that is
/// only staged, only edited or not yet tracked sits in neither and the look
/// passes over it in silence. The second list is how it says so.
///
/// Under `conventional-commits` both halves read commits and no such gap
/// exists; the list is then a prediction of what committing would refuse
/// rather than a hole being closed. Worth saying either way, which is why this
/// does not branch on the intent source.
///
/// Only packages the first answer does not already name reach it: a package
/// that is uncovered either way is one refusal, not two.
///
/// Costs one `git status` beyond [`changed_by_standing`], which is why the
/// pipelines that build a plan keep the cheaper call — the question only
/// matters where someone is about to push.
pub(super) fn changed_by_standing_and_unseen(
    git: &Git,
    workspace: &Workspace,
    files: &[BumpFile],
    from: Option<&str>,
    standing: impl Fn(&Package) -> Standing,
) -> Result<Uncovered, CliError> {
    let classified = classify(workspace, standing);
    let paths = changed_paths(git, from)?;
    let committed = answer(&paths, workspace, files, &classified);
    let outside_head = uncommitted_paths(git).map(|pending| {
        let mut everything = paths;
        everything.extend(pending);
        let if_committed = answer(&everything, workspace, files, &classified);
        let named: BTreeSet<&PackageId> = committed.uncovered.iter().collect();
        Unseen(
            if_committed
                .uncovered
                .iter()
                .filter(|id| !named.contains(id))
                .cloned()
                .collect(),
        )
    });
    Ok(Uncovered {
        committed,
        outside_head,
    })
}

/// Classified once per package and read from the map thereafter: asking twice
/// would let an inconsistent answer report an excluded package as unmanaged,
/// which accuses someone of an omission they did not make.
fn classify(
    workspace: &Workspace,
    standing: impl Fn(&Package) -> Standing,
) -> BTreeMap<PackageId, Standing> {
    workspace
        .packages()
        .map(|package| (package.id().clone(), standing(package)))
        .collect()
}

/// Changed packages the selection keeps, split by whether the config manages
/// them. `unmanaged` never reaches coverage: a package the config cannot plan
/// cannot be covered by intent either, so the two are separate reports. Both
/// are sorted and disjoint.
fn answer(
    paths: &[String],
    workspace: &Workspace,
    files: &[BumpFile],
    classified: &BTreeMap<PackageId, Standing>,
) -> Coverage {
    let kept = packages_for(paths, workspace, &|id: &PackageId| {
        classified.get(id) != Some(&Standing::Excluded)
    });
    let (managed, unmanaged): (Vec<PackageId>, Vec<PackageId>) = kept
        .into_iter()
        .partition(|id| classified.get(id) == Some(&Standing::Managed));
    let managed: BTreeSet<PackageId> = managed.into_iter().collect();
    let covered = covered_packages(files, &managed);
    Coverage {
        uncovered: managed
            .into_iter()
            .filter(|id| !covered.contains(id))
            .collect(),
        unmanaged,
    }
}

fn covered_packages(files: &[BumpFile], changed: &BTreeSet<PackageId>) -> BTreeSet<PackageId> {
    let mut covered = BTreeSet::new();
    let mut empty_file = false;
    for file in files {
        if file.entries.is_empty() {
            empty_file = true;
            continue;
        }
        for (id, _) in &file.entries {
            covered.insert(id.clone());
        }
    }
    if empty_file {
        covered.extend(changed.iter().cloned());
    }
    covered
}

/// Paths changed between `from` and `HEAD`, with intent files dropped.
fn changed_paths(git: &Git, from: Option<&str>) -> Result<Vec<String>, CliError> {
    // At depth 1 the default base resolves to HEAD itself, so the diff comes
    // back empty and every changed package looks covered. `actions/checkout`
    // clones that way by default, which makes this the common CI shape rather
    // than an edge one.
    if super::tags::is_shallow(git)? {
        return Err(CliError::unverified(
            "unverified: shallow clone; changed files cannot be listed against a base that was not fetched — use `fetch-depth: 0`, or `git fetch --unshallow`",
        ));
    }
    let base = resolve_from_ref(git, from).map_err(CliError::from_boxed)?;
    Ok(git
        .paths(Op::ChangedPaths { from: &base })?
        .into_iter()
        .filter(|path| !is_intent_path(path))
        .collect())
}

/// Repository-relative paths the index or the worktree holds that `HEAD` does
/// not — staged, edited and untracked alike — with intent files dropped.
///
/// Porcelain v1 under `-z` writes each entry as its two status letters, a
/// space and the path; a rename or a copy then writes the origin path as its
/// own field (measured, git 2.55: `R  src/renamed.rs\0src/main.rs\0`). Both
/// names are kept, because a file moved out of a package changed that package
/// as much as the one it landed in.
fn uncommitted_paths(git: &Git) -> Result<Vec<String>, CliError> {
    let mut paths = Vec::new();
    let mut origin_follows = false;
    for record in git.paths(Op::UncommittedPaths)? {
        if std::mem::take(&mut origin_follows) {
            paths.push(record);
            continue;
        }
        // A record git wrote in a shape this does not know is a working tree
        // nothing here can compare with `HEAD`; saying so beats attributing a
        // path that was never listed, or passing over one that was.
        let Some((status, path)) = record
            .split_at_checked(3)
            .filter(|(_, path)| !path.is_empty())
        else {
            return Err(CliError::unverified(format!(
                "unverified: `git status --porcelain -z` wrote `{record}`, which is not a status pair and a path, so this tree could not be compared with `HEAD`"
            )));
        };
        origin_follows = status.contains(['R', 'C']);
        paths.push(path.to_owned());
    }
    // A rename whose origin never arrived means the listing stopped early — a
    // child killed mid-write, a full disk. Returning what was read would drop
    // the rest of the tree at exit 0, which is the silence this look exists to
    // break.
    if origin_follows {
        return Err(CliError::unverified(
            "unverified: `git status --porcelain -z` ended after a rename with no origin path, so this tree could not be compared with `HEAD`",
        ));
    }
    paths.retain(|path| !is_intent_path(path));
    Ok(paths)
}

/// Attribute on the full workspace so nested unmanaged packages keep
/// longest-prefix ownership; then drop the ids `kept` rejects.
fn packages_for(
    paths: &[String],
    workspace: &Workspace,
    kept: &impl Fn(&PackageId) -> bool,
) -> BTreeSet<PackageId> {
    let dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    packages_for_paths(paths, &dirs)
        .into_iter()
        .filter(|id| kept(id))
        .collect()
}

fn is_intent_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path == ".changeset" || path.starts_with(".changeset/")
}

#[cfg(test)]
mod tests {
    use oakum::plan::{Ecosystem, ResolvesDependenciesAt};
    use semver::Version;

    use super::super::git::Reply;
    use super::*;

    const STATUS: &str = "status --porcelain -z";
    const DIFF: &str = "diff --name-only";
    const SHALLOW: &str = "rev-parse --is-shallow-repository";
    const RESOLVE: &str = "rev-parse --verify";

    fn heard(stdout: &'static str) -> Result<Vec<String>, CliError> {
        uncommitted_paths(&Git::answering([(STATUS, Reply::said(stdout))]))
    }

    fn one_package() -> Workspace {
        Workspace::new([Package::new(
            PackageId::new(Ecosystem::Cargo, "demo"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )
        .with_manifest_dir("demo")])
        .expect("workspace")
    }

    /// The committed half is already answered by the time the worktree is
    /// read, so a `git status` that cannot run must not take it down. Measured
    /// before this held: exit 1 naming the package became exit 2 naming
    /// nothing.
    ///
    /// Here rather than only in `tests/check.rs`, where the same claim needs a
    /// PATH shim and so is `#[cfg(unix)]` — this is the half that runs on the
    /// Windows job.
    #[test]
    fn a_worktree_that_cannot_be_read_keeps_the_committed_answer() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (RESOLVE, Reply::said("v0.1.0")),
            (DIFF, Reply::said("demo/src/lib.rs\0")),
            (STATUS, Reply::failed(128, "fatal: status is not available")),
        ]);
        let workspace = one_package();
        let uncovered =
            changed_by_standing_and_unseen(&git, &workspace, &[], Some("v0.1.0"), |_| {
                Standing::Managed
            })
            .expect("the committed half still answers");

        assert_eq!(
            uncovered.committed.uncovered,
            [PackageId::new(Ecosystem::Cargo, "demo")],
            "the commits established this before the worktree was read"
        );
        let err = uncovered
            .outside_head
            .expect_err("the worktree could not be read");
        assert_eq!(err.exit_code(), 2, "{err}");
        // The wording too: a child that failed and one that listed only part of
        // the tree are both exit 2, so the class alone cannot tell them apart.
        assert!(err.to_string().contains("failed: exit 128"), "{err}");
    }

    /// Every state that is not a commit reaches the caller the same way: the
    /// look reads commits, so staged, edited and untracked are one class.
    #[test]
    fn each_status_pair_yields_its_path() {
        assert_eq!(
            heard("M  staged.rs\0 M edited.rs\0?? new.rs\0A  added.rs\0").expect("parsed"),
            ["staged.rs", "edited.rs", "new.rs", "added.rs"]
        );
    }

    /// A rename writes the origin path as its own field with no status pair of
    /// its own (measured, git 2.55). Read as an entry it would be attributed by
    /// the characters after its third, which for `src/main.rs` is `/main.rs`.
    ///
    /// Copies travel the same way. `status` does not detect them unless someone
    /// configures it to, so the `C` arm is defence — but narrowing the set to
    /// `R` alone passes every other test, which makes this the only thing
    /// holding it.
    #[test]
    fn a_rename_or_copy_contributes_both_of_its_names() {
        assert_eq!(
            heard("R  beta/moved.rs\0alpha/origin.rs\0 M other.rs\0").expect("parsed"),
            ["beta/moved.rs", "alpha/origin.rs", "other.rs"]
        );
        assert_eq!(
            heard("C  beta/copy.rs\0alpha/origin.rs\0").expect("parsed"),
            ["beta/copy.rs", "alpha/origin.rs"]
        );
    }

    /// The listing stopped after a rename and its origin never arrived — a
    /// child killed mid-write, a full disk. Returning the names that did arrive
    /// would report a partial tree as the whole one at exit 0.
    #[test]
    fn a_rename_with_no_origin_field_is_unverified() {
        let err = heard("R  beta/moved.rs\0").expect_err("refused");
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("no origin path"), "{err}");
    }

    /// Intent is read off disk by the half this one exists to compare against,
    /// so a bump file is not a change to the package it names — the same rule
    /// the committed listing applies.
    #[test]
    fn intent_files_are_not_changes() {
        assert_eq!(
            heard("?? .changeset/demo.md\0 M src/lib.rs\0").expect("parsed"),
            ["src/lib.rs"]
        );
    }

    #[test]
    fn a_clean_tree_lists_nothing() {
        assert_eq!(heard("").expect("parsed"), Vec::<String>::new());
    }

    /// A shape this cannot read is a tree it cannot compare with `HEAD`, which
    /// is `unverified` and not an empty answer: silently dropping the record
    /// would report a clean tree for one it never parsed.
    ///
    /// Both shapes, because they fail different guards: `??` is too short to
    /// split, and `?? ` splits into a status pair and an empty path. Without
    /// the second, the emptiness check is unreachable from any input and reads
    /// as load-bearing while holding nothing.
    #[test]
    fn a_record_without_a_path_is_unverified() {
        for record in ["??\0", "?? \0"] {
            let err = heard(record).expect_err("refused");
            assert_eq!(err.exit_code(), 2, "{record:?}: {err}");
            assert!(
                err.to_string().contains("not a status pair and a path"),
                "{err}"
            );
        }
    }
}
