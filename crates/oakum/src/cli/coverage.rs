use std::collections::{BTreeMap, BTreeSet};

use oakum::changeset::{is_bump_file_name, load_bump_files};
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
    pub(super) outside_head: Result<Worktree, CliError>,
}

/// What reading the working tree added to the committed answer.
#[derive(Debug)]
pub(super) struct Worktree {
    pub(super) unseen: Unseen,
    /// Packages the committed answer calls covered only because intent on disk
    /// says so, where `HEAD`'s own intent does not.
    ///
    /// The mirror of [`Self::unseen`]: there the change is invisible, here the
    /// coverage is. Both halves of the question read different worlds, so both
    /// directions have to be said.
    ///
    /// `Err` when `HEAD`'s intent could not be read, which is a different
    /// failure from an unreadable working tree and has to say so: the run
    /// could not work out what a pull request would be covered by.
    ///
    /// Said, not gated. The committed half is what a pull request is judged on
    /// and it still answers and still refuses, so losing this one costs the
    /// warning and nothing else — the run lands exactly where it did before
    /// this look existed. Decided rather than inherited: a refusal here would
    /// be a refusal on a tree that passes today.
    pub(super) covered_off_disk: Result<Vec<OffDisk>, CliError>,
}

/// A package whose coverage rests on intent a pull request will not receive.
#[derive(Debug)]
pub(super) struct OffDisk {
    pub(super) package: PackageId,
    /// The files it rests on, repository-relative.
    pub(super) files: Vec<String>,
}

impl Worktree {
    /// The report, built here so neither list becomes a bare sequence of ids
    /// beside `Coverage::uncovered` — the one place the three could be
    /// confused for one another.
    pub(super) fn lines(&self, hint: &str) -> Vec<String> {
        let mut lines = self.unseen.lines(hint);
        let off_disk = match &self.covered_off_disk {
            Ok(off_disk) => off_disk,
            Err(err) => {
                lines.push(format!(
                    "the intent `HEAD` carries could not be read, so what a pull request would \
                     be covered by is unknown: {}",
                    err.detail()
                ));
                return lines;
            }
        };
        lines.extend(off_disk.iter().map(|off| {
            let quoted: Vec<String> = off.files.iter().map(|file| format!("`{file}`")).collect();
            let quoted: Vec<&str> = quoted.iter().map(String::as_str).collect();
            format!(
                "{}: covered on disk by {}, not by `HEAD`; commit that, or a pull request gets \
                 this package with nothing covering it",
                off.package,
                super::verdict::named(&quoted)
            )
        }));
        lines
    }
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
    intent_from_disk: bool,
    standing: impl Fn(&Package) -> Standing,
) -> Result<Uncovered, CliError> {
    let classified = classify(workspace, standing);
    let paths = changed_paths(git, from)?;
    let committed = answer(&paths, workspace, files, &classified);
    let outside_head = uncommitted_paths(git).map(|pending| {
        // Intent is read from disk, so a bump file in this listing is not a
        // change to the package it names — it is the other question's subject.
        let changes: Vec<String> = pending
            .into_iter()
            .filter(|path| !is_intent_path(path))
            .collect();
        // Skipped where there is nothing to compare: `conventional-commits`
        // reads intent from commits on both halves — running it there named
        // `.changeset/commits`, the synthetic file that path builds, which is
        // not a path anyone can commit — and no intent on disk means no
        // coverage that could rest on a copy a pull request will not get.
        //
        // Keyed on the disk set rather than on `git status`, which answers a
        // different question than the one the disk reader asks: `read_dir`
        // ignores exclude rules and `skip-worktree`, so a bump file git does
        // not mention still feeds the plan (measured: `.changeset/*.md` in
        // `.git/info/exclude`, and a `skip-worktree` bit, each made the look
        // vanish while the file went on covering).
        let covered_off_disk = if !intent_from_disk || files.is_empty() {
            Ok(Vec::new())
        } else {
            intent_in_head(git, workspace).map(|head_files| {
                resting_on_uncommitted_intent(
                    &head_files,
                    &paths,
                    workspace,
                    files,
                    &classified,
                    &committed,
                )
            })
        };
        let mut everything = paths;
        everything.extend(changes);
        let if_committed = answer(&everything, workspace, files, &classified);
        let already: BTreeSet<&PackageId> = committed.uncovered.iter().collect();
        Worktree {
            unseen: Unseen(
                if_committed
                    .uncovered
                    .iter()
                    .filter(|id| !already.contains(id))
                    .cloned()
                    .collect(),
            ),
            covered_off_disk,
        }
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
/// not — staged, edited and untracked alike, intent files included: which side
/// of the listing one of those falls on is the caller's question, not this
/// one's.
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

/// The intent `HEAD` carries, read from its tree rather than from disk.
///
/// Presence of a path is the wrong question and was measured wrong: a bump file
/// that is committed and then edited to name a second package covers that
/// package here while `HEAD` does not cover it, which is the false green this
/// look exists to close. What a pull request gets is the file's *content* at
/// `HEAD`, so that is what this reads.
fn intent_in_head(git: &Git, workspace: &Workspace) -> Result<Vec<BumpFile>, CliError> {
    let head = git.head()?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for path in git.paths(Op::TreePaths {
        commit: &head,
        dir: BUMP_DIR,
    })? {
        let Some(name) = bump_file_name(&path).filter(|name| is_bump_file_name(name)) else {
            continue;
        };
        pairs.push((
            name.to_owned(),
            git.blob(Op::BlobText {
                commit: &head,
                path: &path,
            })?,
        ));
    }
    // One file at a time, because `load_bump_files` aborts the whole set on a
    // name the workspace does not have — and a pull request that removes a
    // package is exactly that: `HEAD` still carries the bump file naming it.
    // Such a file covers nothing in this workspace, which is an answer rather
    // than a failure to look, so it is dropped and the rest still answers.
    let mut files = Vec::new();
    let mut malformed = Vec::new();
    for (name, body) in &pairs {
        let Ok(loaded) = load_bump_files([(name.as_str(), body.as_str())], workspace) else {
            continue;
        };
        files.extend(loaded.files);
        malformed.extend(loaded.malformed);
    }
    let loaded = oakum::changeset::LoadedBumpFiles { files, malformed };
    // Dropping a file `HEAD` cannot parse is not the quiet-but-safe direction
    // it looks like: where another file already covers the package, the loss
    // shows up as no difference at all, and the run reports nothing while the
    // pull request refuses. What could not be read is said instead.
    if !loaded.malformed.is_empty() {
        let reports: Vec<String> = loaded.malformed.iter().map(ToString::to_string).collect();
        return Err(CliError::unverified(format!(
            "unverified: `HEAD` carries intent oakum cannot parse ({}), so what a pull request \
             would be covered by could not be worked out",
            reports.join("; also ")
        )));
    }
    Ok(loaded.files)
}

/// Which packages the working tree covers that `HEAD` does not.
///
/// Those packages pass here and fail on the pull request, because the file
/// holding them up never leaves the machine.
fn resting_on_uncommitted_intent(
    head_files: &[BumpFile],
    paths: &[String],
    workspace: &Workspace,
    files: &[BumpFile],
    classified: &BTreeMap<PackageId, Standing>,
    committed: &Coverage,
) -> Vec<OffDisk> {
    let already: BTreeSet<&PackageId> = committed.uncovered.iter().collect();
    answer(paths, workspace, head_files, classified)
        .uncovered
        .iter()
        .filter(|id| !already.contains(id))
        .map(|id| OffDisk {
            package: id.clone(),
            // Every disk file covering this package is load-bearing: `id` came
            // out of the answer computed from `head_files`, so none of those
            // covers it. An edited file appears here as readily as one `HEAD`
            // has never seen — both are coverage a pull request will not get.
            files: files
                .iter()
                .filter(|file| covers(file, id))
                .map(|file| format!("{BUMP_DIR}/{}", file.id))
                .collect(),
        })
        .collect()
}

/// Whether a bump file's own content covers `id`: named outright, or by the
/// empty frontmatter that covers whatever changed.
fn covers(file: &BumpFile, id: &PackageId) -> bool {
    file.entries.is_empty() || file.entries.iter().any(|(covered, _)| covered == id)
}

const BUMP_DIR: &str = ".changeset";

fn is_intent_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path == BUMP_DIR
        || path
            .strip_prefix(BUMP_DIR)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A bump file's identity is its bare name, so a listed path becomes one by
/// dropping the directory. `None` for anything nested deeper, which is not a
/// bump file the plan could have read.
fn bump_file_name(path: &str) -> Option<&str> {
    let rest = path.trim_start_matches("./").strip_prefix(BUMP_DIR)?;
    let name = rest.strip_prefix('/')?;
    (!name.contains('/')).then_some(name)
}

#[cfg(test)]
mod tests {
    use oakum::plan::{BumpLevel, Ecosystem, ResolvesDependenciesAt};
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

    /// Reading `HEAD`'s intent can fail on its own — an unreadable blob, a name
    /// the workspace no longer has — and that failure belongs to this half
    /// alone. Folding it into the worktree's outcome said the working tree
    /// could not be read, which was false, and took the packages changed
    /// outside `HEAD` down with it: a finding the previous release printed
    /// simply vanished.
    #[test]
    fn a_head_intent_that_cannot_be_read_keeps_the_rest_of_the_answer() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (RESOLVE, Reply::said("v0.1.0")),
            (DIFF, Reply::said("")),
            (
                STATUS,
                Reply::said("?? .changeset/cover.md\0 M demo/src/lib.rs\0"),
            ),
            (
                "rev-parse HEAD",
                Reply::said("cafebabecafebabecafebabecafebabecafebabe"),
            ),
            ("ls-tree", Reply::said(".changeset/cover.md\0")),
            ("cat-file blob", Reply::failed(128, "fatal: bad object")),
        ]);
        // Two packages so the two halves are about different ones: `demo` is
        // changed outside `HEAD` with nothing covering it, and the disk intent
        // that triggers the `HEAD` read names `other`.
        let workspace = Workspace::new([member("demo"), member("other")]).expect("workspace");
        // Disk intent, because that is what makes the `HEAD` comparison worth
        // asking for; with none there is nothing a pull request could be
        // missing, and the look is correctly skipped.
        let files = [BumpFile {
            id: String::from("cover.md"),
            entries: Vec::from([(PackageId::new(Ecosystem::Cargo, "other"), BumpLevel::Patch)]),
            note: String::from("note"),
        }];
        let uncovered =
            changed_by_standing_and_unseen(&git, &workspace, &files, Some("v0.1.0"), true, |_| {
                Standing::Managed
            })
            .expect("the committed half still answers");
        let worktree = uncovered
            .outside_head
            .expect("the worktree itself read fine");

        let err = worktree
            .covered_off_disk
            .as_ref()
            .expect_err("`HEAD`'s intent could not be read");
        assert!(err.to_string().contains("bad object"), "{err}");
        let lines = worktree.lines("add a bump file");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("intent `HEAD` carries could not be read")),
            "it names what actually failed: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("demo (cargo): changed outside `HEAD`")),
            "and the half that did answer still says so: {lines:?}"
        );
    }

    /// A blob is read exactly as git stored it. `Git::text` trims, which is
    /// right for a hash and wrong here: a bump file whose frontmatter does not
    /// start at byte zero is malformed to the loader that reads the disk copy,
    /// and trimming would let `HEAD`'s copy parse under a different rule than
    /// the copy it is being compared against.
    ///
    /// Malformed travels as a refusal rather than as an empty answer. Dropping
    /// it looks safe and is not: where another file already covers the package,
    /// the loss shows up as no difference at all, so the run says nothing while
    /// the pull request refuses.
    #[test]
    fn head_intent_is_judged_by_the_same_rules_as_the_disk_copy() {
        for body in [
            "\n---\ndemo: patch\n---\nnote\n",
            "not frontmatter at all\n",
        ] {
            let git = Git::answering([
                (
                    "rev-parse HEAD",
                    Reply::said("cafebabecafebabecafebabecafebabecafebabe"),
                ),
                ("ls-tree", Reply::said(".changeset/cover.md\0")),
                ("cat-file blob", Reply::said(body)),
            ]);
            let err = intent_in_head(&git, &one_package())
                .expect_err("a body the disk loader would refuse is refused here");
            // The class, which is what travels; the caller reports it without
            // gating, so no process ever exits on it.
            assert_eq!(err.exit_code(), 2, "{body:?}: {err}");
            assert!(err.to_string().contains("cannot parse"), "{body:?}: {err}");
        }
    }

    /// A bump file is named by its bare filename, so only a direct child of the
    /// directory can be one.
    #[test]
    fn only_a_direct_child_of_the_bump_directory_is_a_bump_file() {
        assert_eq!(bump_file_name(".changeset/demo.md"), Some("demo.md"));
        assert_eq!(bump_file_name("./.changeset/demo.md"), Some("demo.md"));
        assert_eq!(bump_file_name(".changeset/nested/demo.md"), None);
        assert_eq!(bump_file_name(".changesetish/demo.md"), None);
        assert_eq!(bump_file_name("src/lib.rs"), None);
    }

    fn member(name: &str) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )
        .with_manifest_dir(name)
    }

    fn one_package() -> Workspace {
        Workspace::new([member("demo")]).expect("workspace")
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
            changed_by_standing_and_unseen(&git, &workspace, &[], Some("v0.1.0"), true, |_| {
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

    /// Which side of the listing a path falls on. Intent is read off disk by
    /// the half this one compares against, so a bump file is never a change to
    /// the package it names; it is the other question's evidence, which is why
    /// the parser hands it through instead of dropping it.
    #[test]
    fn intent_is_told_from_change() {
        for path in [".changeset/demo.md", "./.changeset/demo.md", ".changeset"] {
            assert!(is_intent_path(path), "{path}");
        }
        for path in [
            "src/lib.rs",
            ".changesetish/demo.md",
            "a/.changeset/demo.md",
        ] {
            assert!(!is_intent_path(path), "{path}");
        }
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
