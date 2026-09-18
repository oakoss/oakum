//! `migrate`'s look for files that gate on the old bump-file directory.
//!
//! Behind the git seam, so every outcome is reachable from a script; the
//! renderer lives in [`super::migrate_output`].

use oakum::detect::ReleaseTool;

use super::git::{Git, Op};
use super::repository;
use super::CliError;

/// bumpy's bump-file directory, without a trailing slash: a gate is as likely
/// to be written `grep '^\.bumpy'` as `-- '.bumpy/*.md'`, and searching for the
/// slashed form alone misses the first. [`Op::FilesMentioning`] appends the
/// slash for the pathspec that excludes the directory itself.
///
/// changesets and knope both use `.changeset/`, which oakum adopts in place, so
/// a gate pointed at it still finds files there.
const BUMPY_DIR: &str = ".bumpy";

/// What the repository's own files say about the old bump-file directory.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GateLook {
    /// Tracked files naming it, outside `.changeset/` and outside the directory
    /// itself. Empty means the look ran and found none — which is reported, not
    /// passed over in silence, because the look sees only tracked files.
    Found(Vec<String>),
    /// The index lists no file and git reports no commit, so there is nothing a
    /// tracked gate could be hiding in. An index that is merely missing is
    /// [`Self::Failed`]: measured, a repository whose HEAD carries a gate
    /// answers `ls-files` with silence once `.git/index` is deleted, and
    /// calling that "nothing tracked" would exit 0 over a live gate.
    ///
    /// `rev-parse --verify --quiet HEAD` answers an unborn branch and a HEAD
    /// pointing at a vanished ref alike, so the line reports what git said
    /// rather than asserting the repository is empty.
    NothingTracked,
    /// The look failed. Never folded into an empty [`Self::Found`]: a gate
    /// nobody looked for is not a gate that is not there.
    Failed(String),
    /// The source tool's bump files already live in `.changeset/`, so there is
    /// no old directory for anything to be pointed at. Not "we skipped it".
    NothingToRepoint,
}

impl GateLook {
    /// The one outcome that sets the exit code, carrying why. A method rather
    /// than a bare `match` at the one site that raises: which arm refuses is a
    /// property of the look, not a fact the caller re-derives. The step that
    /// reports the same failure still matches every arm, and is exhaustive, so
    /// the compiler guards it.
    pub(super) fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(why) => Some(why),
            Self::Found(_) | Self::NothingTracked | Self::NothingToRepoint => None,
        }
    }
}

/// Files that gate on the old tool's bump-file directory (`okm-404.24`).
///
/// From the claude-plugins field record: a `PreToolUse` hook grepped
/// `git diff --cached -- '.bumpy/*.md'` for the plugin name, so once
/// `.changeset/` went live a commit carrying a valid oakum bump file was
/// rejected with a message telling the developer to use bumpy. Repointing it is
/// the reader's work; noticing the gate exists is cheap and nothing else does
/// it.
pub(super) fn find_bump_file_gates(
    repo: &repository::Repository,
    detections: &[oakum::detect::Detection],
) -> GateLook {
    if !detections
        .iter()
        .any(|hit| hit.tool() == ReleaseTool::Bumpy)
    {
        return GateLook::NothingToRepoint;
    }
    Git::at_repository(repo)
        .map_err(CliError::from_boxed)
        .and_then(|git| gate_look(&git))
        .unwrap_or_else(|err| GateLook::Failed(err.detail()))
}

/// What the index says about the old directory. A look that failed is `Err`,
/// so the caller is the one place that spells a failure as
/// [`GateLook::Failed`], rather than each arm spelling it its own way. Only
/// `Found` and `NothingTracked` come back on the `Ok` side; the signature does
/// not enforce that, so no arm here builds the other two.
///
/// # Errors
///
/// A child that could not look, or an index that lists nothing while HEAD has
/// commits.
fn gate_look(git: &Git) -> Result<GateLook, CliError> {
    if let Some(paths) = git.matched_paths(Op::FilesMentioning { dir: BUMPY_DIR })? {
        return Ok(GateLook::Found(paths));
    }
    // `git grep` answers "no match" and "I searched no files" with the same
    // exit 1 and the same silence. Asking what there was to search separates
    // them; a `git` wrapper that exits 1 without a diagnostic still reaches
    // the wrong one, which no question can fix from here.
    if !git.paths(Op::TrackedFiles)?.is_empty() {
        return Ok(GateLook::Found(Vec::new()));
    }
    // An empty index over a repository that has commits is a broken index,
    // not an empty repository: the files are in HEAD and a gate among them was
    // never searched. Only a repository with no commit at all has nothing to
    // hide.
    if git.predicate(Op::RefExists { reference: "HEAD" })? {
        return Err(CliError::new(
            "git's index lists no file while HEAD has commits, so the index is missing or unbuilt and a gate among the committed files was not searched",
        ));
    }
    Ok(GateLook::NothingTracked)
}

#[cfg(test)]
mod tests {
    use super::super::git::{Git, Reply};
    use super::{gate_look, GateLook};

    const GREP: &str = "grep --name-only";
    const TRACKED: &str = "ls-files";
    const HEAD: &str = "rev-parse --verify";

    #[test]
    fn a_tracked_file_naming_the_directory_is_found() {
        let git = Git::answering([(
            GREP,
            Reply::said(".claude/hooks/require-bump-file.sh\0.github/workflows/gate.yml\0"),
        )]);
        assert_eq!(
            gate_look(&git).expect("looked"),
            GateLook::Found(Vec::from([
                String::from(".claude/hooks/require-bump-file.sh"),
                String::from(".github/workflows/gate.yml"),
            ]))
        );
        assert_eq!(git.asked(), [GREP]);
    }

    /// Only a wrapper that failed to look exits 0 here in silence, so the
    /// tracked files are never consulted: the question went unanswered.
    #[test]
    fn a_grep_that_exited_zero_without_naming_a_file_is_not_an_empty_find() {
        let git = Git::answering([(GREP, Reply::exactly(Some(0), b"", b""))]);
        let err = gate_look(&git).expect_err("a look nobody completed is not an empty find");
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("without answering"), "{err}");
        assert_eq!(git.asked(), [GREP]);
    }

    /// `git grep` says "no match" and "nothing to search" alike; the tracked
    /// files answer which, and a populated index makes it a real empty find.
    #[test]
    fn no_match_over_tracked_files_is_an_empty_find() {
        let git = Git::answering([
            (GREP, Reply::absent()),
            (TRACKED, Reply::said("README.md\0")),
        ]);
        assert_eq!(
            gate_look(&git).expect("looked"),
            GateLook::Found(Vec::new())
        );
        assert_eq!(git.asked(), [GREP, TRACKED]);
    }

    /// Scripted out of call order: the fake answers by name, which is what
    /// makes the `asked` assertion an ordering claim rather than an echo.
    #[test]
    fn an_empty_index_with_no_commit_is_nothing_tracked() {
        let git = Git::answering([
            (HEAD, Reply::absent()),
            (TRACKED, Reply::said("")),
            (GREP, Reply::absent()),
        ]);
        assert_eq!(gate_look(&git).expect("looked"), GateLook::NothingTracked);
        assert_eq!(git.asked(), [GREP, TRACKED, HEAD]);
    }

    /// A later child that died or only warned is a failure to look, never the
    /// answer the branch it sits on would have given: an empty find and
    /// `NothingTracked` both need every question answered.
    #[test]
    fn a_later_child_that_died_or_only_warned_stops_the_look() {
        let warned = || Reply::warned("warning: unable to access '/etc/gitconfig'");
        for (op, reply) in [
            (TRACKED, Reply::was_signalled()),
            (TRACKED, warned()),
            (HEAD, Reply::was_signalled()),
            (HEAD, warned()),
        ] {
            let mut script = Vec::from([(GREP, Reply::absent())]);
            if op == HEAD {
                script.push((TRACKED, Reply::said("")));
            }
            let expected: Vec<&str> = script.iter().map(|(op, _)| *op).chain([op]).collect();
            script.push((op, reply));
            let git = Git::answering(script);
            gate_look(&git).expect_err("a child that did not answer stops the look");
            assert_eq!(git.asked(), expected, "no child after the one that failed");
        }
    }

    /// The files are in HEAD and a gate among them was never searched, so
    /// this is a failure to look, never "nothing tracked".
    #[test]
    fn an_empty_index_over_commits_is_a_failure_to_look() {
        let git = Git::answering([
            (GREP, Reply::absent()),
            (TRACKED, Reply::said("")),
            (HEAD, Reply::said("cafe")),
        ]);
        let err = gate_look(&git).expect_err("a broken index is not an empty repository");
        assert!(
            err.detail()
                .contains("index lists no file while HEAD has commits"),
            "{err}"
        );
    }

    /// A noisy exit 1 is a wrapper that failed to look, not "no match". It
    /// must not fall through to the tracked-files question, whose answer
    /// would stand in for a search that never happened.
    #[test]
    fn a_diagnosed_exit_one_is_a_failure_not_an_empty_search() {
        let git = Git::answering([
            (GREP, Reply::failed(1, "fatal: unable to read index")),
            (TRACKED, Reply::said("README.md\0")),
        ]);
        let err = gate_look(&git).expect_err("a diagnosed failure");
        assert!(err.detail().contains("unable to read index"), "{err}");
        assert_eq!(git.asked(), [GREP], "no second child after a failed look");
    }
}
