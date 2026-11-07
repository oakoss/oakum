//! `migrate`'s look for files that gate on the old bump-file directory.
//!
//! The index look is behind the git seam, so every outcome is reachable from a
//! script; the hooks are read from disk. The renderer lives in
//! [`super::migrate_output`].

use std::fs;
use std::io;
use std::path::Path;

use oakum::detect::ReleaseTool;

use super::git::{Git, Op};
use super::quoted;
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
    Looked {
        index: IndexLook,
        hooks: HookLook,
    },
    /// No git to ask, so neither look ran.
    Failed(String),
    /// The source tool's bump files already live in `.changeset/`, so there is
    /// no old directory for anything to be pointed at. Not "we skipped it".
    NothingToRepoint,
}

/// What git's index says about the old bump-file directory.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IndexLook {
    /// Tracked files naming it, outside `.changeset/` and outside the directory
    /// itself. Empty means the look ran and found none — which is reported, not
    /// passed over in silence, because the look sees only tracked files.
    ///
    /// `unmerged` lists the conflicted files the search skipped; any makes the
    /// look unverified, since a gate there was never read.
    Found {
        named: Vec<String>,
        unmerged: Vec<String>,
    },
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
}

/// The hooks git runs, which the index look cannot see: git never tracks
/// `.git/hooks/`, the classic home of a commit gate.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HookLook {
    /// `dir` as git names it; `named` the hooks naming the old directory, and
    /// `unread` each hook that could not be read, with why.
    Read {
        dir: String,
        named: Vec<String>,
        unread: Vec<String>,
    },
    Failed(String),
}

impl GateLook {
    /// The one outcome that sets the exit code, carrying why. A method rather
    /// than a bare `match` at the one site that raises: which arm refuses is a
    /// property of the look, not a fact the caller re-derives. The step that
    /// reports the same failure still matches every arm, and is exhaustive, so
    /// the compiler guards it.
    pub(super) fn failure(&self) -> Option<String> {
        let (index, hooks) = match self {
            Self::Failed(why) => return Some(why.clone()),
            Self::NothingToRepoint => return None,
            Self::Looked { index, hooks } => (index, hooks),
        };
        let index = match index {
            IndexLook::Failed(why) => Some(why.clone()),
            IndexLook::Found { unmerged, .. } if !unmerged.is_empty() => Some(format!(
                "`git grep --cached` skips unmerged index entries, so {} was not searched",
                quoted(unmerged)
            )),
            IndexLook::Found { .. } | IndexLook::NothingTracked => None,
        };
        let hooks = match hooks {
            HookLook::Failed(why) => Some(why.clone()),
            HookLook::Read { unread, .. } if !unread.is_empty() => {
                Some(format!("could not read the hook {}", unread.join(", ")))
            }
            HookLook::Read { .. } => None,
        };
        let why: Vec<String> = index.into_iter().chain(hooks).collect();
        (!why.is_empty()).then(|| why.join("; "))
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
    let (git, root) = match Git::at_repository(repo).and_then(|git| {
        let root = repo.ambient_path()?.to_path_buf();
        Ok((git, root))
    }) {
        Ok(found) => found,
        Err(err) => return GateLook::Failed(CliError::from_boxed(err).detail()),
    };
    GateLook::Looked {
        index: gate_look(&git).unwrap_or_else(|err| IndexLook::Failed(err.detail())),
        hooks: hook_look(&git, &root),
    }
}

/// What the index says about the old directory. A look that failed is `Err`,
/// so the caller is the one place that spells a failure as
/// [`IndexLook::Failed`], rather than each arm spelling it its own way.
///
/// # Errors
///
/// A child that could not look, or an index that lists nothing while HEAD has
/// commits.
fn gate_look(git: &Git) -> Result<IndexLook, CliError> {
    let named = git.matched_paths(Op::FilesMentioning { dir: BUMPY_DIR })?;
    let unmerged = unmerged_paths(git)?;
    if named.is_some() || !unmerged.is_empty() {
        return Ok(IndexLook::Found {
            named: named.unwrap_or_default(),
            unmerged,
        });
    }
    // `git grep` answers "no match" and "I searched no files" with the same
    // exit 1 and the same silence. Asking what there was to search separates
    // them; a `git` wrapper that exits 1 without a diagnostic still reaches
    // the wrong one, which no question can fix from here.
    if !git.paths(Op::TrackedFiles)?.is_empty() {
        return Ok(IndexLook::Found {
            named: Vec::new(),
            unmerged: Vec::new(),
        });
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
    Ok(IndexLook::NothingTracked)
}

/// githooks(5)'s events as git 2.56.0 installs it: git runs a hook only under
/// one of these names, and calls any other an unknown hook event.
const HOOK_EVENTS: [&str; 28] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
    "fsmonitor-watchman",
    "p4-changelist",
    "p4-prepare-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
    "post-index-change",
];

fn hook_look(git: &Git, root: &Path) -> HookLook {
    match git.line(Op::HooksDir) {
        Ok(dir) => read_hooks(&root.join(&dir), dir),
        Err(err) => HookLook::Failed(err.detail()),
    }
}

/// Reads every hook git would run from `path`, without running one. Symlinks
/// are followed, since a hook manager may link a hook elsewhere.
fn read_hooks(path: &Path, dir: String) -> HookLook {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        // Nothing there, or a file: git runs no hook from either.
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return HookLook::Read {
                dir,
                named: Vec::new(),
                unread: Vec::new(),
            }
        }
        Err(err) => return HookLook::Failed(format!("could not list the hooks in `{dir}`: {err}")),
    };
    let mut named = Vec::new();
    let mut unread = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return HookLook::Failed(format!("could not list the hooks in `{dir}`: {err}"))
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !HOOK_EVENTS.contains(&name.as_str()) {
            continue;
        }
        let shown = format!("{dir}/{name}");
        let text = match fs::metadata(entry.path()) {
            // A symlink to nothing, which git cannot find to run.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Ok(meta) if !meta.is_file() || !executable(&meta) => continue,
            Ok(_) => fs::read(entry.path()),
            Err(err) => Err(err),
        };
        match text {
            Ok(text) if names_dir(&text, BUMPY_DIR) => named.push(shown),
            Ok(_) => {}
            Err(err) => unread.push(format!("`{shown}` ({err})")),
        }
    }
    named.sort();
    unread.sort();
    HookLook::Read { dir, named, unread }
}

/// Git on Unix ignores a hook without an execute bit.
#[cfg(unix)]
fn executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> bool {
    true
}

/// The token rule [`Op::FilesMentioning`] gives `git grep`: `dir` followed by
/// a byte that cannot continue a name, or by the end of the text.
fn names_dir(text: &[u8], dir: &str) -> bool {
    let dir = dir.as_bytes();
    text.windows(dir.len()).enumerate().any(|(at, window)| {
        window == dir
            && text
                .get(at + dir.len())
                .is_none_or(|next| !next.is_ascii_alphanumeric() && *next != b'_')
    })
}

/// Each conflicted path once, from [`Op::UnmergedFiles`]' per-stage records.
fn unmerged_paths(git: &Git) -> Result<Vec<String>, CliError> {
    let mut paths: Vec<String> = Vec::new();
    for record in git.paths(Op::UnmergedFiles { dir: BUMPY_DIR })? {
        let Some((_, path)) = record.split_once('\t') else {
            return Err(CliError::unverified(format!(
                "git listed an unmerged entry without a path: {record:?}"
            )));
        };
        if !paths.iter().any(|seen| seen == path) {
            paths.push(path.to_owned());
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::super::git::{Git, Reply};
    use super::{gate_look, hook_look, names_dir, read_hooks, HookLook, IndexLook};
    use crate::test_fixture::Fixture;
    use std::fs;
    use std::path::Path;

    const GREP: &str = "grep --name-only";
    const UNMERGED: &str = "ls-files --unmerged";
    const TRACKED: &str = "ls-files";
    const HEAD: &str = "rev-parse --verify";

    fn found(named: &[&str], unmerged: &[&str]) -> IndexLook {
        IndexLook::Found {
            named: named.iter().map(ToString::to_string).collect(),
            unmerged: unmerged.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn a_tracked_file_naming_the_directory_is_found() {
        let git = Git::answering([
            (
                GREP,
                Reply::said(".claude/hooks/require-bump-file.sh\0.github/workflows/gate.yml\0"),
            ),
            (UNMERGED, Reply::said("")),
        ]);
        assert_eq!(
            gate_look(&git).expect("looked"),
            found(
                &[
                    ".claude/hooks/require-bump-file.sh",
                    ".github/workflows/gate.yml"
                ],
                &[]
            )
        );
        assert_eq!(git.asked(), [GREP, UNMERGED]);
    }

    /// One record per stage, so a three-way conflict lists its path three
    /// times; a path carrying a tab keeps it, since the prefix has none.
    #[test]
    fn each_unmerged_path_is_named_once() {
        let git = Git::answering([
            (GREP, Reply::absent()),
            (
                UNMERGED,
                Reply::said(
                    "100644 aaaa 1\tgate.sh\x00100644 bbbb 2\tgate.sh\x00100644 cccc 3\tgate.sh\x00100644 dddd 2\ta\tb.sh\0",
                ),
            ),
        ]);
        assert_eq!(
            gate_look(&git).expect("looked"),
            found(&[], &["gate.sh", "a\tb.sh"])
        );
        assert_eq!(
            git.asked(),
            [GREP, UNMERGED],
            "unmerged entries are tracked"
        );
    }

    #[test]
    fn an_unmerged_record_without_a_path_stops_the_look() {
        let git = Git::answering([
            (GREP, Reply::absent()),
            (UNMERGED, Reply::said("100644 aaaa 1 gate.sh\0")),
        ]);
        let err = gate_look(&git).expect_err("an unreadable record is not an empty one");
        assert_eq!(err.exit_code(), 2, "{err}");
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
            (UNMERGED, Reply::said("")),
            (TRACKED, Reply::said("README.md\0")),
        ]);
        assert_eq!(gate_look(&git).expect("looked"), found(&[], &[]));
        assert_eq!(git.asked(), [GREP, UNMERGED, TRACKED]);
    }

    /// Scripted out of call order: the fake answers by name, which is what
    /// makes the `asked` assertion an ordering claim rather than an echo.
    #[test]
    fn an_empty_index_with_no_commit_is_nothing_tracked() {
        let git = Git::answering([
            (HEAD, Reply::absent()),
            (TRACKED, Reply::said("")),
            (UNMERGED, Reply::said("")),
            (GREP, Reply::absent()),
        ]);
        assert_eq!(gate_look(&git).expect("looked"), IndexLook::NothingTracked);
        assert_eq!(git.asked(), [GREP, UNMERGED, TRACKED, HEAD]);
    }

    /// A later child that died or only warned is a failure to look, never the
    /// answer the branch it sits on would have given: an empty find and
    /// `NothingTracked` both need every question answered.
    #[test]
    fn a_later_child_that_died_or_only_warned_stops_the_look() {
        let warned = || Reply::warned("warning: unable to access '/etc/gitconfig'");
        for (op, reply) in [
            (UNMERGED, Reply::was_signalled()),
            (UNMERGED, warned()),
            (TRACKED, Reply::was_signalled()),
            (TRACKED, warned()),
            (HEAD, Reply::was_signalled()),
            (HEAD, warned()),
        ] {
            let mut script = Vec::from([(GREP, Reply::absent())]);
            if op != UNMERGED {
                script.push((UNMERGED, Reply::said("")));
            }
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
            (UNMERGED, Reply::said("")),
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

    /// The same boundary the `git grep` pattern draws, so the two looks agree
    /// on what a mention is.
    #[test]
    fn a_hook_names_the_directory_at_a_token_boundary() {
        for text in [
            "ls .bumpy/",
            "grep '^\\.bumpy'",
            ".bumpy",
            "x .bumpy\n",
            ".bumpy-\u{e9}",
        ] {
            assert!(names_dir(text.as_bytes(), ".bumpy"), "{text:?}");
        }
        for text in ["the .bumpyrc file", "my-app.bumpy_x", "xbumpy/", ""] {
            assert!(!names_dir(text.as_bytes(), ".bumpy"), "{text:?}");
        }
    }

    /// Only git's newline goes: a directory named with a trailing space is
    /// another directory than the trimmed name.
    #[test]
    fn the_hooks_directory_keeps_its_trailing_whitespace() {
        let git = Git::answering([("rev-parse --git-path hooks", Reply::said("my-hooks \n"))]);
        let root = Fixture::new("migrate-gates", "hooks-space");
        let HookLook::Read { dir, .. } = hook_look(&git, &root) else {
            panic!("an absent directory holds no hook");
        };
        assert_eq!(dir, "my-hooks ");
    }

    #[test]
    fn a_hooks_directory_that_cannot_be_named_is_a_failed_look() {
        let git = Git::answering([(
            "rev-parse --git-path hooks",
            Reply::failed(128, "fatal: not a git repository"),
        )]);
        let HookLook::Failed(why) = hook_look(&git, Path::new("/unread")) else {
            panic!("a hooks directory git would not name was not read");
        };
        assert!(why.contains("not a git repository"), "{why}");
    }

    /// Only what git would run is read: a `.sample` and a subdirectory are
    /// neither, whatever they say. Listed out of order, reported sorted.
    #[test]
    fn the_hooks_read_skips_samples_and_subdirectories() {
        let root = Fixture::new("migrate-gates", "hooks");
        let dir = root.join("hooks");
        fs::create_dir_all(dir.join("lib")).expect("dirs");
        for name in ["pre-push", "pre-commit", "commit-msg.sample", "lib/helper"] {
            fs::write(dir.join(name), "ls .bumpy/\n").expect("hook");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        fs::write(dir.join("post-merge"), "echo done\n").expect("hook");
        assert_eq!(
            read_hooks(&dir, String::from("hooks")),
            HookLook::Read {
                dir: String::from("hooks"),
                named: Vec::from([
                    String::from("hooks/pre-commit"),
                    String::from("hooks/pre-push"),
                ]),
                unread: Vec::new(),
            }
        );
        assert_eq!(
            read_hooks(&root.join("absent"), String::from("absent")),
            HookLook::Read {
                dir: String::from("absent"),
                named: Vec::new(),
                unread: Vec::new(),
            },
            "git runs no hook from a directory that is not there"
        );
    }
}
