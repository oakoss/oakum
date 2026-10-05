//! Restore already-landed files if a later write or delete fails.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;

use cap_std::fs::Dir;

use super::fs::{
    create_consume_marker, mark_rolled_back, open_read_only, own_staging_files, repo_path_display,
    stage_aside, write_file_exclusive, write_file_via_rename, STAGING_CLAIM,
};
use super::CliError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlannedWrite {
    path: PathBuf,
    original: String,
    next: String,
    /// File did not exist; rollback must remove it, not write an empty original.
    created: bool,
}

impl PlannedWrite {
    pub(super) fn new(path: PathBuf, original: impl Into<String>, next: impl Into<String>) -> Self {
        Self {
            path,
            original: original.into(),
            next: next.into(),
            created: false,
        }
    }

    pub(super) fn create(path: PathBuf, next: impl Into<String>) -> Self {
        Self {
            path,
            original: String::new(),
            next: next.into(),
            created: true,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn original(&self) -> &str {
        &self.original
    }

    pub(super) fn next(&self) -> &str {
        &self.next
    }

    pub(super) fn set_next(&mut self, next: impl Into<String>) {
        self.next = next.into();
    }

    pub(super) fn created(&self) -> bool {
        self.created
    }

    fn set_created(&mut self) {
        self.created = true;
    }
}

/// Accumulated writes keyed by path. Read-through returns staged text when present.
pub(super) struct WriteSet {
    entries: BTreeMap<PathBuf, PlannedWrite>,
}

impl WriteSet {
    pub(super) fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub(super) fn extend(&mut self, writes: impl IntoIterator<Item = PlannedWrite>) {
        for write in writes {
            self.stage(write);
        }
    }

    pub(super) fn source_text(
        &self,
        dir: &Dir,
        path: &Path,
    ) -> Result<(String, String), Box<dyn std::error::Error>> {
        if let Some(write) = self.entries.get(path) {
            return Ok((write.original().to_owned(), write.next().to_owned()));
        }
        let text = read_text(dir, path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is missing", repo_path_display(path)),
            )
        })?;
        Ok((text.clone(), text))
    }

    pub(super) fn put_write(&mut self, path: PathBuf, original: String, next: String) {
        match self.entries.get_mut(&path) {
            Some(write) => write.set_next(next),
            None => {
                self.entries
                    .insert(path.clone(), PlannedWrite::new(path, original, next));
            }
        }
    }

    pub(super) fn writes(&self) -> Vec<PlannedWrite> {
        self.entries.values().cloned().collect()
    }

    fn stage(&mut self, write: PlannedWrite) {
        let path = write.path().to_owned();
        match self.entries.get_mut(&path) {
            Some(existing) => {
                existing.set_next(write.next().to_owned());
                if write.created() {
                    existing.set_created();
                }
            }
            None => {
                self.entries.insert(path, write);
            }
        }
    }
}

pub(super) fn read_text(
    dir: &Dir,
    path: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut file = match open_read_only(dir, path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(io::Error::new(
                err.kind(),
                format!("failed to open {}: {err}", repo_path_display(path)),
            )
            .into());
        }
    };
    let meta = file.metadata().map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to inspect {}: {err}", repo_path_display(path)),
        )
    })?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", repo_path_display(path)),
        )
        .into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to read {}: {err}", repo_path_display(path)),
        )
    })?;
    Ok(Some(text))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlannedDelete {
    path: PathBuf,
}

impl PlannedDelete {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// # Errors
///
/// Already-landed files are restored to `original` before the error is returned.
#[cfg(test)]
pub(super) fn commit_writes(dir: &Dir, writes: &[PlannedWrite]) -> Result<(), WriteSetFailure> {
    // No deletes, so nothing is staged and nothing can be left unremoved.
    commit_write_set(dir, writes, &[]).map(|committed| drop(committed.unremoved))
}

/// One of the filesystem verbs a write set performs, forward or in rollback.
/// Rollback verbs are their own so a test can let a write land and refuse
/// only its restore.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    /// Creating the consume marker, keyed by its directory.
    Mark,
    /// Marking a kept consume marker as rolled back, keyed by its directory.
    MarkRolledBack,
    Create,
    Write,
    /// Moving a consumed file aside to its staging name.
    Stage,
    /// Removing a staged file once every write and stage has landed.
    Remove,
    /// Putting a landed write or a staged file back.
    Restore,
    /// Removing a file this run created.
    Discard,
}

/// Refusals a test scripts, keyed by verb and path the way the git fake keys
/// answers by operation: each is claimed once, everything unscripted reaches
/// the real filesystem, and an unclaimed refusal is one the code was right
/// not to reach — [`Self::unclaimed`] says which. The shipping path carries
/// none. A refused verb leaves the disk as the fault it stands in for would:
/// `Stage` has a real driver (a read-only directory, tested below) and the
/// disk states match; `Discard`, `Remove` and both `Restore`s need one
/// filesystem permission for the landing and another for the undoing, which
/// no runner's flags express mid-run.
struct Faults {
    #[cfg(test)]
    scripted: Mutex<Vec<Option<(Verb, PathBuf)>>>,
}

impl Faults {
    fn none() -> Self {
        Self {
            #[cfg(test)]
            scripted: Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    fn refusing<'a>(entries: impl IntoIterator<Item = (Verb, &'a str)>) -> Self {
        Self {
            scripted: Mutex::new(
                entries
                    .into_iter()
                    .map(|(verb, path)| Some((verb, PathBuf::from(path))))
                    .collect(),
            ),
        }
    }

    /// The verb's real effect, unless a test refused it.
    fn attempt<T, E: From<io::Error>>(
        &self,
        verb: Verb,
        path: &Path,
        op: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        match self.refuse(verb, path) {
            Some(refused) => Err(refused.into()),
            None => op(),
        }
    }

    #[cfg(test)]
    fn unclaimed(&self) -> Vec<(Verb, PathBuf)> {
        self.scripted
            .lock()
            .expect("the faults are not shared")
            .iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// The scripted refusal for this verb on this path, claimed.
    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "the shipping Faults has nothing to consult"
        )
    )]
    fn refuse(&self, verb: Verb, path: &Path) -> Option<io::Error> {
        #[cfg(test)]
        {
            let mut scripted = self.scripted.lock().expect("the faults are not shared");
            let refused = scripted
                .iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|(v, p)| *v == verb && p == path))
                .and_then(Option::take)?;
            Some(io::Error::other(format!(
                "refused by the test: {:?} {}",
                refused.0,
                repo_path_display(&refused.1)
            )))
        }
        #[cfg(not(test))]
        {
            let _ = (verb, path);
            None
        }
    }
}

/// A write set that landed, and the staged files it could not remove. Each
/// holds text the changelog already has; the next `check` refuses them.
#[must_use = "staged files that landed but could not be removed must be reported"]
#[derive(Debug)]
pub(super) struct Committed {
    unremoved: Vec<(PathBuf, String)>,
}

impl Committed {
    /// # Errors
    ///
    /// Names every staged file that could not be removed, and why.
    pub(super) fn into_result(self) -> Result<(), CliError> {
        if self.unremoved.is_empty() {
            return Ok(());
        }
        let mut message = format!(
            "every write landed and every bump file was consumed, but {} file(s) left from \
             consuming them could not be removed; remove them:",
            self.unremoved.len()
        );
        for (path, err) in self.unremoved.iter().take(HEAD_FILES) {
            let path = repo_path_display(path);
            let _ = write!(message, "\n  {} ({})", Printable(&path), Printable(err));
        }
        match self.unremoved.len().saturating_sub(HEAD_FILES) {
            0 => {}
            rest => {
                let _ = write!(message, "\n  … {rest} more");
            }
        }
        Err(CliError::new(message))
    }
}

/// A later failure moves staged deletes back, then restores writes.
///
/// # Errors
///
/// Already-landed files are restored to `original` before the error is returned.
pub(super) fn commit_write_set(
    dir: &Dir,
    writes: &[PlannedWrite],
    deletes: &[PlannedDelete],
) -> Result<Committed, WriteSetFailure> {
    commit_write_set_under(dir, writes, deletes, &Faults::none())
}

fn commit_write_set_under(
    dir: &Dir,
    writes: &[PlannedWrite],
    deletes: &[PlannedDelete],
    faults: &Faults,
) -> Result<Committed, WriteSetFailure> {
    if let Some(path) = overlapping_path(writes, deletes) {
        return Err(WriteSetFailure::refused(format!(
            "write-set path appears in both writes and deletes: {}",
            repo_path_display(path)
        )));
    }
    let marker = match deletes.first() {
        Some(first) => {
            let sub = first.path.parent().unwrap_or(Path::new(""));
            let consumes: Vec<&Path> = deletes.iter().map(|delete| delete.path.as_path()).collect();
            let created: Result<PathBuf, Box<dyn std::error::Error>> =
                faults.attempt(Verb::Mark, sub, || {
                    create_consume_marker(dir, sub, &consumes).map_err(Into::into)
                });
            Some(created.map_err(|err| WriteSetFailure::refused(err.to_string()))?)
        }
        None => None,
    };
    let marker = marker.as_deref();
    let mut done_writes = Vec::new();
    for write in writes {
        if !write.created && write.original == write.next {
            continue;
        }
        // Sampled before the attempt so a create that lost a race to an
        // existing file is not reported as something this run stranded.
        let existed_before = write.created && dir.metadata(&write.path).is_ok();
        let verb = if write.created {
            Verb::Create
        } else {
            Verb::Write
        };
        let write_result: Result<(), Box<dyn std::error::Error>> =
            faults.attempt(verb, &write.path, || {
                if write.created {
                    write_file_exclusive(dir, &write.path, &write.next).map_err(Into::into)
                } else {
                    write_file_via_rename(dir, &write.path, &write.next)
                }
            });
        if let Err(err) = write_result {
            let attempt = if write.created {
                Attempt::Create {
                    path: &write.path,
                    existed_before,
                }
            } else {
                Attempt::Replace(&write.path)
            };
            return Err(rollback(
                dir,
                &done_writes,
                &[],
                marker,
                Some(attempt),
                err.as_ref(),
                faults,
            ));
        }
        done_writes.push(write);
    }
    // Consumed files are renamed aside, not unlinked, until every write has
    // landed: a run killed or failed in between leaves their bytes on disk.
    let mut staged = Vec::new();
    for delete in deletes {
        let moved: Result<PathBuf, Box<dyn std::error::Error>> =
            faults.attempt(Verb::Stage, &delete.path, || {
                stage_aside(dir, &delete.path).map_err(Into::into)
            });
        match moved {
            Ok(staging) => staged.push(Staged {
                path: &delete.path,
                staging,
            }),
            Err(err) => {
                return Err(rollback(
                    dir,
                    &done_writes,
                    &staged,
                    marker,
                    Some(Attempt::Delete(&delete.path)),
                    err.as_ref(),
                    faults,
                ));
            }
        }
    }
    // Every write and stage landed, so the consume is complete and nothing
    // rolls back from here.
    let mut unremoved = Vec::new();
    for item in staged {
        // Keyed by the consumed path: the staging name is not known in advance.
        let removed = faults.attempt(Verb::Remove, item.path, || dir.remove_file(&item.staging));
        if let Err(err) = removed {
            unremoved.push((item.staging, err.to_string()));
        }
    }
    // Removed last, so it outlives every set-aside file it accounts for.
    if let Some(marker) = marker {
        if let Err(err) = dir.remove_file(marker) {
            unremoved.push((marker.to_path_buf(), err.to_string()));
        }
    }
    Ok(Committed { unremoved })
}

/// Whether a rollback left something the next `version` must not plan over.
fn marker_must_stay(left_changed: &[LeftChanged]) -> bool {
    left_changed.iter().any(|entry| {
        matches!(
            entry,
            LeftChanged::Unrestored { .. }
                | LeftChanged::Created { .. }
                | LeftChanged::Stranded { .. }
        )
    })
}

/// A consumed file moved aside, and where it went.
struct Staged<'a> {
    path: &'a Path,
    staging: PathBuf,
}

fn overlapping_path<'a>(
    writes: &'a [PlannedWrite],
    deletes: &'a [PlannedDelete],
) -> Option<&'a Path> {
    deletes.iter().find_map(|delete| {
        writes
            .iter()
            .any(|write| write.path == delete.path)
            .then_some(delete.path.as_path())
    })
}

/// The step that failed, which decides both the directory to sweep and whether
/// a file it created may still be on disk. A bare path loses the second.
#[derive(Clone, Copy)]
enum Attempt<'a> {
    Create {
        path: &'a Path,
        existed_before: bool,
    },
    Replace(&'a Path),
    Delete(&'a Path),
}

impl<'a> Attempt<'a> {
    fn path(self) -> &'a Path {
        match self {
            Self::Create { path, .. } | Self::Replace(path) | Self::Delete(path) => path,
        }
    }
}

/// One file the tree kept, and which way. Rendering stays here so a caller can
/// ask what was left without parsing the sentence it prints.
#[derive(Debug)]
enum LeftChanged {
    /// A restore that reported failure: the file holds neither its original
    /// nor the planned content reliably.
    Unrestored { path: String, err: String },
    /// A consumed file whose rename back failed: a later run cannot see it and
    /// versions without it, and `staging` may hold its only copy.
    Stranded {
        path: String,
        staging: String,
        err: String,
    },
    /// A create that landed and whose removal failed.
    Created { path: String },
    /// A staging file this run wrote and could not remove.
    StagingLeak { path: String },
    /// The consume marker, kept because the rollback left something changed.
    MarkerKept { path: String, marked: bool },
}

impl fmt::Display for LeftChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrestored { path, err } => {
                write!(
                    f,
                    "{} (restore failed: {})",
                    Printable(path),
                    Printable(err)
                )
            }
            Self::Stranded { path, staging, err } => write!(
                f,
                "{} (moved to {}, and could not be moved back: {})",
                Printable(path),
                Printable(staging),
                Printable(err)
            ),
            Self::Created { path } => {
                write!(f, "{} (created and could not be removed)", Printable(path))
            }
            Self::StagingLeak { path } => write!(f, "{} ({STAGING_CLAIM})", Printable(path)),
            Self::MarkerKept { path, marked: true } => write!(
                f,
                "{} (kept: `oakum version` refuses while it is here; remove it once the other \
                 files listed are put right)",
                Printable(path)
            ),
            // Unmarked, the next `version` reads it as a run whose writes
            // landed, and its advice would remove bump files.
            Self::MarkerKept {
                path,
                marked: false,
            } => write!(
                f,
                "{} (kept, but it could not be marked as rolled back, so the next `oakum version` \
                 will misread it: keep none of this run's writes, remove no bump file, put the \
                 other files listed right, then remove it)",
                Printable(path)
            ),
        }
    }
}

/// A write set that failed, and everything it could not put back.
///
/// Empty lists mean one of two things, which this type does not distinguish:
/// the run was refused before anything was attempted, or every restore
/// reported success and every swept directory could be read. The second is
/// weaker than a byte-identical tree, because rollback writes `original` back
/// without re-reading to confirm it; file identity, mode, and a plan gone stale
/// since the read are outside the claim. Nothing outside this module can read
/// the lists, so the conflation is inert — splitting them is `okm-2ppr.13`.
#[derive(Debug)]
pub(super) struct WriteSetFailure {
    cause: String,
    left_changed: Vec<LeftChanged>,
    unswept: Vec<String>,
}

impl WriteSetFailure {
    /// Refused before any write was attempted, so there is nothing to put back.
    fn refused(cause: String) -> Self {
        Self {
            cause,
            left_changed: Vec::new(),
            unswept: Vec::new(),
        }
    }

    fn new(cause: String, mut left_changed: Vec<LeftChanged>, mut unswept: Vec<String>) -> Self {
        // Sorted on the rendered line, which is what the previous `Vec<String>`
        // sorted; ordering by variant would reorder the report.
        left_changed.sort_by_key(ToString::to_string);
        unswept.sort();
        Self {
            cause,
            left_changed,
            unswept,
        }
    }
}

/// Past this many entries the list stops and says how many it left out, so the
/// advice after it stays on screen.
const HEAD_FILES: usize = 5;

/// Anything that could rewrite what the terminal shows is escaped rather than
/// sent on: a bump file, and the path naming it, can arrive from a
/// contributor's pull request. `is_control` alone is not enough — a bidi
/// override reverses the rendering of everything after it and is not a
/// control character.
struct Printable<'a>(&'a str);

impl Printable<'_> {
    fn shown_as_written(ch: char) -> bool {
        match ch {
            // Neither moves a cursor nor hides text, and the joiners are
            // orthographically required in Persian, Urdu and Indic scripts.
            '\t' | '"' | '\'' | '\\' | '\u{200C}' | '\u{200D}' => true,
            // Rust's own printability table, which escapes Cc, Cf, Cs, Co, Cn,
            // Zl, Zp and grapheme-extend. Wider than the threat needs — a
            // combining mark is ordinary text — but every hand-written range
            // list tried here missed a carrier, tag characters and U+2028
            // among them. `okm-2ppr.14` moves this to one seam with a policy.
            _ => ch.escape_debug().count() == 1,
        }
    }
}

impl fmt::Display for Printable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            if Self::shown_as_written(ch) {
                write!(f, "{ch}")?;
            } else {
                write!(f, "{}", ch.escape_debug())?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for WriteSetFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Printable(&self.cause))?;
        if !self.left_changed.is_empty() {
            write!(f, "\n{} file(s) left changed:", self.left_changed.len())?;
            for entry in self.left_changed.iter().take(HEAD_FILES) {
                write!(f, "\n  {entry}")?;
            }
            match self.left_changed.len().saturating_sub(HEAD_FILES) {
                0 => {}
                1 => write!(f, "\n  … 1 more")?,
                rest => write!(f, "\n  … {rest} more")?,
            }
        }
        // A consumed file that could not be moved back is the only entry a
        // reader must act on before re-running: the next run cannot see it,
        // consumes what is left, and reports success at a version nobody asked for.
        let stranded: Vec<(&str, &str)> = self
            .left_changed
            .iter()
            .filter_map(|entry| match entry {
                LeftChanged::Stranded { path, staging, .. } => {
                    Some((staging.as_str(), path.as_str()))
                }
                _ => None,
            })
            .collect();
        if !stranded.is_empty() {
            let pronoun = if stranded.len() == 1 { "it" } else { "them" };
            write!(
                f,
                "\nmove {pronoun} back before re-running; a later run cannot see {pronoun} and \
                 will version without {pronoun}:"
            )?;
            // Debug-quoted: a name holding a newline would otherwise split one
            // path across two lines, and these are meant to be pasted.
            for (staging, path) in stranded.iter().take(HEAD_FILES) {
                write!(f, "\n  {staging:?} -> {path:?}")?;
            }
            match stranded.len().saturating_sub(HEAD_FILES) {
                0 => {}
                1 => write!(f, "\n  … 1 more, named in the list above")?,
                rest => write!(f, "\n  … {rest} more, named in the list above")?,
            }
        }
        // Its own list: a directory oakum could not read supports no claim
        // about the tree, and counting it among changed files sends a reader
        // hunting for damage that may not exist.
        if !self.unswept.is_empty() {
            write!(
                f,
                "\n{} director{} could not be checked for staging files:",
                self.unswept.len(),
                if self.unswept.len() == 1 { "y" } else { "ies" }
            )?;
            for entry in &self.unswept {
                write!(f, "\n  {}", Printable(entry))?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for WriteSetFailure {}

fn rollback(
    dir: &Dir,
    done_writes: &[&PlannedWrite],
    staged: &[Staged<'_>],
    marker: Option<&Path>,
    attempted: Option<Attempt<'_>>,
    err: &dyn std::error::Error,
    faults: &Faults,
) -> WriteSetFailure {
    let cause = err.to_string();
    let mut left_changed = Vec::new();
    let mut stranded = BTreeSet::new();
    for item in staged.iter().rev() {
        let restored = faults.attempt(Verb::Restore, item.path, || {
            dir.rename(&item.staging, dir, item.path)
        });
        if let Err(restore_err) = restored {
            stranded.insert(repo_path_display(&item.staging));
            left_changed.push(LeftChanged::Stranded {
                path: repo_path_display(item.path),
                staging: repo_path_display(&item.staging),
                err: restore_err.to_string(),
            });
        }
    }
    for write in done_writes.iter().rev() {
        let restore = if write.created {
            faults
                .attempt(Verb::Discard, &write.path, || dir.remove_file(&write.path))
                .or_else(|err| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(err)
                    }
                })
                .map_err(|err| {
                    format!("failed to remove {}: {err}", repo_path_display(&write.path))
                })
        } else {
            faults
                .attempt(Verb::Restore, &write.path, || {
                    write_file_via_rename(dir, &write.path, &write.original)
                })
                .map_err(|err| err.to_string())
        };
        if let Err(restore_err) = restore {
            left_changed.push(LeftChanged::Unrestored {
                path: repo_path_display(&write.path),
                err: restore_err,
            });
        }
    }
    // A create that landed and could not be cleaned up is the one leftover that
    // is not a staging file, so the sweep cannot find it. No test drives this:
    // it needs `write_file_exclusive` to fail after `create_new` landed and
    // its own cleanup to fail too, both below the seam `Faults` gives.
    if let Some(Attempt::Create {
        path,
        existed_before: false,
    }) = attempted
    {
        if dir.metadata(path).is_ok() {
            left_changed.push(LeftChanged::Created {
                path: repo_path_display(path),
            });
        }
    }
    // The marker goes only once everything is back. Kept, it makes the next
    // `version` refuse rather than plan over a write this rollback left, and
    // says it was a rollback so no advice tells anyone to keep the writes.
    if let Some(marker) = marker {
        let path = repo_path_display(marker);
        stranded.insert(path.clone());
        if marker_must_stay(&left_changed) {
            let sub = marker.parent().unwrap_or(Path::new(""));
            let rewritten: Result<(), Box<dyn std::error::Error>> =
                faults.attempt(Verb::MarkRolledBack, sub, || {
                    mark_rolled_back(dir, marker).map_err(Into::into)
                });
            left_changed.push(LeftChanged::MarkerKept {
                path,
                marked: rewritten.is_ok(),
            });
        } else if dir.remove_file(marker).is_err() {
            left_changed.push(LeftChanged::StagingLeak { path });
        }
    }
    let (leaked, unswept) = own_staging_leftovers(dir, done_writes, staged, attempted);
    // A stranded file's staging name is the last copy of its text, named
    // above with the rename that restores it; the leak advice would remove it.
    left_changed.extend(leaked.into_iter().filter(
        |entry| !matches!(entry, LeftChanged::StagingLeak { path } if stranded.contains(path)),
    ));
    WriteSetFailure::new(cause, left_changed, unswept)
}

/// Staging files this run left behind, which `write_file_via_rename` tries to
/// remove and cannot report when that removal is what failed. Every directory
/// this run wrote, deleted from, or attempted is swept: rollback restores a
/// delete through the same writer, and the write that failed never reaches
/// `done_writes`. Returns the leaks found and, separately, the directories that
/// could not be read: the fault that strands a staging file is the kind that
/// also blocks the listing, so silence would hide a leak exactly when there is
/// one — but an unreadable directory is no evidence of a changed file either.
fn own_staging_leftovers(
    dir: &Dir,
    done_writes: &[&PlannedWrite],
    staged: &[Staged<'_>],
    attempted: Option<Attempt<'_>>,
) -> (Vec<LeftChanged>, Vec<String>) {
    let subs: BTreeSet<String> = done_writes
        .iter()
        .map(|write| write.path.as_path())
        .chain(staged.iter().map(|item| item.path))
        .chain(attempted.map(Attempt::path))
        .map(|path| match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => repo_path_display(parent),
            _ => String::from("."),
        })
        .collect();
    let mut leaked = Vec::new();
    let mut unswept = Vec::new();
    for sub in &subs {
        match own_staging_files(dir, sub) {
            Ok(found) => leaked.extend(
                found
                    .into_iter()
                    .map(|path| LeftChanged::StagingLeak { path }),
            ),
            Err(err) => unswept.push(format!("{sub} ({err})")),
        }
    }
    (leaked, unswept)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use cap_std::fs::Dir;

    #[cfg(unix)]
    use crate::test_fixture::expect_refused;
    use crate::test_fixture::Fixture;

    use super::STAGING_CLAIM;
    use super::{
        commit_write_set, commit_write_set_under, commit_writes, create_consume_marker,
        marker_must_stay, Committed, Faults, LeftChanged, PlannedDelete, PlannedWrite, Printable,
        Verb, WriteSet, WriteSetFailure, HEAD_FILES,
    };

    fn scratch(label: &str) -> Fixture {
        Fixture::new("write-set", label)
    }

    #[test]
    fn read_through_returns_staged_text_without_reading_disk() {
        let root = scratch("read-through");
        fs::write(root.join("manifest.toml"), "disk").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let mut write_set = WriteSet::new();
        write_set.put_write(
            PathBuf::from("manifest.toml"),
            "disk".to_owned(),
            "staged".to_owned(),
        );
        let (original, next) = write_set
            .source_text(&dir, Path::new("manifest.toml"))
            .expect("read-through");
        assert_eq!(original, "disk");
        assert_eq!(next, "staged");
        assert_eq!(
            fs::read_to_string(root.join("manifest.toml")).unwrap(),
            "disk"
        );
    }

    #[test]
    fn repeated_staging_keeps_the_first_original() {
        let mut write_set = WriteSet::new();
        let path = PathBuf::from("Cargo.toml");
        write_set.put_write(path.clone(), "0".to_owned(), "1".to_owned());
        write_set.put_write(path.clone(), "ignored".to_owned(), "2".to_owned());
        write_set.extend([PlannedWrite::new(path.clone(), "also-ignored", "3")]);
        let write = write_set
            .writes()
            .into_iter()
            .find(|write| write.path() == path.as_path())
            .expect("staged");
        assert_eq!(write.original(), "0");
        write_set.extend([PlannedWrite::create(
            PathBuf::from("CHANGELOG.md"),
            "# Changelog\n",
        )]);
        write_set.extend([PlannedWrite::new(
            PathBuf::from("CHANGELOG.md"),
            "ignored",
            "# Changelog\n\n## 0.1.0\n",
        )]);
        let write = write_set
            .writes()
            .into_iter()
            .find(|write| write.path() == Path::new("CHANGELOG.md"))
            .expect("staged");
        assert!(write.created());
        assert_eq!(write.next(), "# Changelog\n\n## 0.1.0\n");
    }

    #[test]
    fn staging_order_does_not_change_the_final_write_set() {
        let inherited = PlannedWrite::new(PathBuf::from("a.txt"), "a0", "a1");
        let member = PlannedWrite::new(PathBuf::from("b.txt"), "b0", "b1");
        let lock = PlannedWrite::new(PathBuf::from("Cargo.lock"), "l0", "l1");

        let mut forward = WriteSet::new();
        forward.extend([inherited.clone(), member.clone()]);
        forward.extend([lock.clone()]);

        let mut reverse = WriteSet::new();
        reverse.extend([lock.clone()]);
        reverse.extend([member.clone(), inherited.clone()]);

        assert_eq!(forward.writes(), reverse.writes());
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_text_is_not_written() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("skip-same");
        fs::write(root.join("keep.txt"), "same").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let mut perms = fs::metadata(&root).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&root, perms).unwrap();

        let result = commit_writes(
            &dir,
            &[PlannedWrite::new(PathBuf::from("keep.txt"), "same", "same")],
        );
        let mut restore = fs::metadata(&root).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&root, restore).unwrap();
        result.expect("skip");

        assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "same");
    }

    #[cfg(unix)]
    #[test]
    fn later_write_failure_restores_earlier_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("three-restore");
        fs::create_dir_all(root.join("c")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("b.txt"), "B0").unwrap();
        fs::write(root.join("c/file.txt"), "C0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let blocked = root.join("c");
        let mut perms = fs::metadata(&blocked).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&blocked, perms).unwrap();

        let err = commit_writes(
            &dir,
            &[
                PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1"),
                PlannedWrite::new(PathBuf::from("b.txt"), "B0", "B1"),
                PlannedWrite::new(PathBuf::from("c/file.txt"), "C0", "C1"),
            ],
        );
        let mut restore = fs::metadata(&blocked).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&blocked, restore).unwrap();
        expect_refused(err, "third write");

        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A0");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "B0");
        assert_eq!(fs::read_to_string(root.join("c/file.txt")).unwrap(), "C0");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_set_names_the_staging_file_it_could_not_remove() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("staging-leak");
        fs::create_dir_all(root.join("blocked")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("blocked/b.txt"), "B0").unwrap();
        // In the failed write's own directory, which only `attempted` reaches —
        // `done_writes` contributes the root. Planted rather than provoked: a
        // rename whose cleanup also fails is a fault inside
        // `write_file_via_rename`, below the seam `Faults` gives.
        let leaked = format!("blocked/.b.txt.oakum-write.{}.0.0", std::process::id());
        fs::write(root.join(&leaked), "partial").unwrap();

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let blocked = root.join("blocked");
        let mut perms = fs::metadata(&blocked).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&blocked, perms).unwrap();

        let result = commit_write_set(
            &dir,
            &[
                PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1"),
                PlannedWrite::new(PathBuf::from("blocked/b.txt"), "B0", "B1"),
            ],
            &[],
        );
        let mut restore = fs::metadata(&blocked).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&blocked, restore).unwrap();

        let err = expect_refused(result, "blocked write").to_string();
        assert!(err.contains("failed to stage `blocked/b.txt`"), "{err}");
        assert!(err.contains("1 file(s) left changed:"), "{err}");
        assert!(
            err.contains(&format!("{leaked} ({STAGING_CLAIM})")),
            "{err}"
        );
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A0");
    }

    #[cfg(unix)]
    #[test]
    fn a_concurrent_runs_staging_file_is_not_this_failures_to_name() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("staging-other-pid");
        fs::create_dir_all(root.join("blocked")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("blocked/b.txt"), "B0").unwrap();
        let other = format!("blocked/.b.txt.oakum-write.{}.0.0", std::process::id() + 1);
        fs::write(root.join(&other), "another run").unwrap();

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let blocked = root.join("blocked");
        let mut perms = fs::metadata(&blocked).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&blocked, perms).unwrap();

        let result = commit_write_set(
            &dir,
            &[
                PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1"),
                PlannedWrite::new(PathBuf::from("blocked/b.txt"), "B0", "B1"),
            ],
            &[],
        );
        let mut restore = fs::metadata(&blocked).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&blocked, restore).unwrap();

        let err = expect_refused(result, "blocked write").to_string();
        assert!(!err.contains("left changed"), "{err}");
        assert!(!err.contains(&other), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_named_not_counted_clean() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("staging-unswept");
        fs::create_dir_all(root.join("blind")).unwrap();
        fs::create_dir_all(root.join("locked")).unwrap();
        fs::write(root.join("blind/b.txt"), "B0").unwrap();
        fs::write(root.join("locked/c.txt"), "C0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();

        // `blind` is writable and not readable; `locked` is readable and not
        // writable, so some write fails on either platform — provided the
        // process is not root, which bypasses both bits. Which write fails
        // differs: Linux lands the `blind` write and reaches the sweep through
        // `done_writes`, macOS refuses it and reaches the sweep through
        // `attempted`. The sweep cannot read `blind` in both.
        let blind = root.join("blind");
        let blind_mode = fs::metadata(&blind).unwrap().permissions().mode();
        let mut perms = fs::metadata(&blind).unwrap().permissions();
        perms.set_mode(0o333);
        fs::set_permissions(&blind, perms).unwrap();
        let locked = root.join("locked");
        let locked_mode = fs::metadata(&locked).unwrap().permissions().mode();
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(&locked, perms).unwrap();

        let result = commit_write_set(
            &dir,
            &[
                PlannedWrite::new(PathBuf::from("blind/b.txt"), "B0", "B1"),
                PlannedWrite::new(PathBuf::from("locked/c.txt"), "C0", "C1"),
            ],
            &[],
        );
        for (path, mode) in [(&blind, blind_mode), (&locked, locked_mode)] {
            let mut restore = fs::metadata(path).unwrap().permissions();
            restore.set_mode(mode);
            fs::set_permissions(path, restore).unwrap();
        }

        let err = expect_refused(result, "blocked write").to_string();
        assert!(
            err.contains("1 directory could not be checked for staging files:"),
            "{err}"
        );
        assert!(err.contains("\n  blind ("), "{err}");
        // An unreadable directory is not a changed file and must not be counted
        // as one.
        assert!(!err.contains("left changed"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_delete_sweeps_its_own_directory_and_the_restored_ones() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("delete-sweep");
        fs::create_dir_all(root.join("gone")).unwrap();
        fs::write(root.join("gone/first.md"), "F0").unwrap();
        fs::write(root.join("gone/second.md"), "S0").unwrap();
        // The delete arm reaches no directory through `done_writes`, so without
        // the failing delete's own path this file is unreachable by the sweep.
        let leaked = format!("gone/.first.md.oakum-write.{}.0.0", std::process::id());
        fs::write(root.join(&leaked), "partial").unwrap();
        // A second leak in the directory of a delete that was staged, which
        // only the staged list reaches.
        fs::create_dir_all(root.join("done")).unwrap();
        fs::write(root.join("done/ok.md"), "O0").unwrap();
        let restored_leak = format!("done/.ok.md.oakum-write.{}.0.0", std::process::id());
        fs::write(root.join(&restored_leak), "partial").unwrap();
        // A target whose own name carries the mark: the pid must be read from
        // the mark the writer appended, which is the last one.
        let nested = format!(
            "done/.a.oakum-write.9.md.oakum-write.{}.0.0",
            std::process::id()
        );
        fs::write(root.join(&nested), "partial").unwrap();

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let gone = root.join("gone");
        let original_mode = fs::metadata(&gone).unwrap().permissions().mode();
        let mut perms = fs::metadata(&gone).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(&gone, perms).unwrap();

        let result = commit_write_set(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from("done/ok.md")),
                PlannedDelete::new(PathBuf::from("gone/second.md")),
            ],
        );
        let mut restore = fs::metadata(&gone).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&gone, restore).unwrap();

        let err = expect_refused(result, "blocked delete").to_string();
        assert!(
            err.contains("failed to move `gone/second.md` aside"),
            "{err}"
        );
        for named in [&leaked, &restored_leak, &nested] {
            assert!(err.contains(&format!("{named} ({STAGING_CLAIM})")), "{err}");
        }
        assert_eq!(fs::read_to_string(root.join("done/ok.md")).unwrap(), "O0");
        assert_eq!(
            fs::read_to_string(root.join("gone/second.md")).unwrap(),
            "S0"
        );
    }

    #[test]
    fn a_created_file_with_an_empty_body_is_still_written() {
        let root = scratch("create-empty");
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        // `create` leaves `original` empty, so the unchanged-text skip would
        // drop this write and report success over a file that never appeared.
        commit_writes(
            &dir,
            &[PlannedWrite::create(PathBuf::from("empty.txt"), "")],
        )
        .expect("create");
        assert_eq!(fs::read_to_string(root.join("empty.txt")).unwrap(), "");
    }

    #[test]
    fn a_create_that_loses_to_an_existing_file_is_not_reported_as_stranded() {
        let root = scratch("create-race");
        fs::write(root.join("taken.txt"), "theirs").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();

        let err = commit_write_set(
            &dir,
            &[PlannedWrite::create(PathBuf::from("taken.txt"), "ours")],
            &[],
        )
        .expect_err("create over an existing file")
        .to_string();
        assert!(err.contains("failed to create `taken.txt`"), "{err}");
        assert!(!err.contains("left changed"), "{err}");
        assert_eq!(
            fs::read_to_string(root.join("taken.txt")).unwrap(),
            "theirs"
        );
    }

    #[test]
    fn a_clean_rollback_lists_nothing() {
        let failure = WriteSetFailure::new(
            String::from("failed to replace `a.toml`: nope"),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(failure.to_string(), "failed to replace `a.toml`: nope");
    }

    #[test]
    fn every_unrestored_path_gets_its_own_line_in_a_stable_order() {
        let failure = WriteSetFailure::new(
            String::from("failed to replace `a.toml`: nope"),
            // Rendered order and variant order disagree here: `Created` is
            // declared second but renders first. Ordering by variant instead
            // of by line would swap these two.
            vec![
                LeftChanged::Unrestored {
                    path: String::from("z.md"),
                    err: String::from("x"),
                },
                LeftChanged::Created {
                    path: String::from("a.md"),
                },
            ],
            Vec::new(),
        );
        assert_eq!(
            failure.to_string(),
            "failed to replace `a.toml`: nope\n2 file(s) left changed:\n  a.md (created and could not be removed)\n  z.md (restore failed: x)"
        );
    }

    fn stranded(name: &str) -> LeftChanged {
        LeftChanged::Stranded {
            path: format!(".changeset/{name}.md"),
            staging: format!(".changeset/.{name}.md.oakum-write.1.2.0"),
            err: String::from("nope"),
        }
    }

    #[test]
    fn a_stranded_file_is_named_with_where_its_text_is() {
        let failure = WriteSetFailure::new(
            String::from("failed: x"),
            vec![stranded("gone")],
            Vec::new(),
        );
        assert_eq!(
            failure.to_string(),
            "failed: x\n1 file(s) left changed:\n  .changeset/gone.md (moved to .changeset/.gone.md.oakum-write.1.2.0, and could not be moved back: nope)\nmove it back before re-running; a later run cannot see it and will version without it:\n  \".changeset/.gone.md.oakum-write.1.2.0\" -> \".changeset/gone.md\""
        );
    }

    /// One restore failing is usually every restore failing, so naming only
    /// the first would lose the rest to the next run.
    #[test]
    fn every_stranded_file_is_listed_until_the_cap() {
        let entries: Vec<LeftChanged> = (0..HEAD_FILES + 2)
            .map(|n| stranded(&format!("s{n}")))
            .collect();
        let text = WriteSetFailure::new(String::from("nope"), entries, Vec::new()).to_string();
        assert!(text.contains("move them back before re-running"), "{text}");
        assert!(text.contains("\".changeset/s0.md\""), "{text}");
        assert!(
            text.ends_with("… 2 more, named in the list above"),
            "{text}"
        );
    }

    /// The cause is the one line always printed, and it carries a path a pull
    /// request can name.
    #[test]
    fn the_cause_line_is_escaped() {
        let failure = WriteSetFailure::refused(String::from(
            "failed to delete .changeset/z\u{1b}[2Jz\u{202e}.md",
        ));
        let text = failure.to_string();
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{202e}'), "{text:?}");
    }

    /// The carriers a hand-written range list kept missing.
    #[test]
    fn invisible_carriers_are_escaped_and_ordinary_text_is_not() {
        for hidden in ['\u{E0041}', '\u{2060}', '\u{2028}', '\u{00AD}', '\u{FEFF}'] {
            let rendered = Printable(&hidden.to_string()).to_string();
            assert!(
                !rendered.contains(hidden),
                "U+{:04X} reached raw",
                hidden as u32
            );
        }
        // A reader has to recognize the file, so ordinary script stays readable.
        for shown in ['中', '👩', 'é', '\u{200D}', '\u{200C}', '\t'] {
            let rendered = Printable(&shown.to_string()).to_string();
            assert!(
                rendered.contains(shown),
                "U+{:04X} was mangled",
                shown as u32
            );
        }
    }

    #[test]
    fn past_the_file_cap_the_entry_list_stops_and_says_so() {
        let entries: Vec<LeftChanged> = (0..HEAD_FILES + 3)
            .map(|n| LeftChanged::Unrestored {
                path: format!("{n}.md"),
                err: String::from("x"),
            })
            .collect();
        let text = WriteSetFailure::new(String::from("nope"), entries, Vec::new()).to_string();
        assert!(text.contains("\n  … 3 more"), "{text}");
    }

    /// The entry line carries a path and error a pull request can supply.
    #[test]
    fn a_path_and_error_in_the_entry_line_are_escaped() {
        let failure = WriteSetFailure::new(
            String::from("nope"),
            vec![LeftChanged::Unrestored {
                path: String::from("a\u{1b}[31m.md"),
                err: String::from("b\u{202e}ad"),
            }],
            vec![String::from("dir\u{1b}[2J")],
        );
        let text = failure.to_string();
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{202e}'), "{text:?}");
    }

    /// A file left holding the wrong bytes is still on disk, so a later run
    /// sees it. Only a consumed file needs the caller to put something back.
    #[test]
    fn an_unrestored_file_asks_for_nothing_to_be_moved_back() {
        let failure = WriteSetFailure::new(
            String::from("failed to replace `a.toml`: nope"),
            vec![LeftChanged::Unrestored {
                path: String::from("a.toml"),
                err: String::from("x"),
            }],
            Vec::new(),
        );
        assert!(
            !failure.to_string().contains("back before re-running"),
            "{failure}"
        );
    }

    #[test]
    fn overlapping_write_and_delete_is_an_error() {
        let root = scratch("overlap");
        fs::write(root.join("same.txt"), "old").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = commit_write_set(
            &dir,
            &[PlannedWrite::new(PathBuf::from("same.txt"), "old", "new")],
            &[PlannedDelete::new(PathBuf::from("same.txt"))],
        )
        .expect_err("overlap");
        assert!(
            err.to_string()
                .contains("write-set path appears in both writes and deletes: same.txt"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(root.join("same.txt")).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn delete_failure_restores_writes_and_earlier_deletes() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("delete-restore");
        fs::create_dir_all(root.join("blocked")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("keep.md"), "K0").unwrap();
        fs::write(root.join("blocked/gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let blocked = root.join("blocked");
        let mut perms = fs::metadata(&blocked).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&blocked, perms).unwrap();

        let err = commit_write_set(
            &dir,
            &[PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1")],
            &[
                PlannedDelete::new(PathBuf::from("keep.md")),
                PlannedDelete::new(PathBuf::from("blocked/gone.md")),
            ],
        );
        let mut restore = fs::metadata(&blocked).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&blocked, restore).unwrap();
        expect_refused(err, "blocked delete");

        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A0");
        assert_eq!(fs::read_to_string(root.join("keep.md")).unwrap(), "K0");
        assert_eq!(
            fs::read_to_string(root.join("blocked/gone.md")).unwrap(),
            "G0"
        );
        assert!(
            staging_names(&root).is_empty(),
            "{:?}",
            staging_names(&root)
        );
    }

    /// Every staging name, of either mark, directly under `root`.
    fn staging_names(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.contains(".oakum-write.") || name.contains(".oakum-consume."))
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn later_write_failure_removes_a_created_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("create-restore");
        fs::create_dir_all(root.join("c")).unwrap();
        fs::write(root.join("c/file.txt"), "C0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let blocked = root.join("c");
        let mut perms = fs::metadata(&blocked).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&blocked, perms).unwrap();

        let err = commit_writes(
            &dir,
            &[
                PlannedWrite::create(PathBuf::from("CHANGELOG.md"), "# Changelog\n"),
                PlannedWrite::new(PathBuf::from("c/file.txt"), "C0", "C1"),
            ],
        );
        let mut restore = fs::metadata(&blocked).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&blocked, restore).unwrap();
        expect_refused(err, "second write");

        assert!(!root.join("CHANGELOG.md").exists());
        assert_eq!(fs::read_to_string(root.join("c/file.txt")).unwrap(), "C0");
    }

    /// A staged file whose move back fails keeps its text at the staging name,
    /// and the report names that name with the move that restores it, not the
    /// leak advice to remove it.
    #[test]
    fn a_staged_file_whose_move_back_is_refused_is_named_where_it_is() {
        // Under `.changeset/`, where bump files live: the stranded name must
        // match the path the leak sweep reports, or the filter misses it.
        let root = scratch("stage-restore-refused");
        let changeset = root.join(".changeset");
        fs::create_dir_all(&changeset).unwrap();
        fs::write(changeset.join("keep.md"), "K0").unwrap();
        fs::write(changeset.join("gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([
            (Verb::Stage, ".changeset/gone.md"),
            (Verb::Restore, ".changeset/keep.md"),
        ]);

        let err = commit_write_set_under(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from(".changeset/keep.md")),
                PlannedDelete::new(PathBuf::from(".changeset/gone.md")),
            ],
            &faults,
        )
        .expect_err("the second stage is refused")
        .to_string();

        let (markers, names): (Vec<String>, Vec<String>) = staging_names(&changeset)
            .into_iter()
            .partition(|name| name.starts_with(".version.oakum-consume."));
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(
            markers.len(),
            1,
            "a stranded file keeps the marker: {markers:?}"
        );
        let staging = format!(".changeset/{}", names[0]);
        assert_eq!(fs::read_to_string(root.join(&staging)).unwrap(), "K0");
        assert!(!changeset.join("keep.md").exists());
        assert_eq!(fs::read_to_string(changeset.join("gone.md")).unwrap(), "G0");
        assert!(
            err.contains("refused by the test: Stage .changeset/gone.md"),
            "{err}"
        );
        assert!(err.contains("2 file(s) left changed:"), "{err}");
        assert!(
            err.contains(&format!(
                ".changeset/keep.md (moved to {staging}, and could not be moved back"
            )) && err.contains(&format!("{staging:?} -> \".changeset/keep.md\"")),
            "{err}"
        );
        assert!(!err.contains(STAGING_CLAIM), "{err}");
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    /// A name near `NAME_MAX` stages under a shortened name rather than being
    /// refused, both when the consume lands and when it rolls back. Unix only:
    /// measured against this filesystem's 255-byte `NAME_MAX`.
    #[cfg(unix)]
    #[test]
    fn a_name_near_name_max_stages_under_a_shortened_name() {
        let root = scratch("stage-long-name");
        // A two-byte character straddles the cut, which must land on a
        // character boundary rather than panic.
        let long = format!("{}é{}.md", "a".repeat(199), "x".repeat(49));
        fs::write(root.join(&long), "L0").unwrap();
        fs::write(root.join("gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Stage, "gone.md")]);

        commit_write_set_under(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from(&long)),
                PlannedDelete::new(PathBuf::from("gone.md")),
            ],
            &faults,
        )
        .expect_err("the second stage is refused");
        assert_eq!(fs::read_to_string(root.join(&long)).unwrap(), "L0");
        assert!(
            staging_names(&root).is_empty(),
            "{:?}",
            staging_names(&root)
        );

        commit_write_set(&dir, &[], &[PlannedDelete::new(PathBuf::from(&long))])
            .expect("the long name is consumed")
            .into_result()
            .expect("nothing left");
        assert!(!root.join(&long).exists());
        assert!(
            staging_names(&root).is_empty(),
            "{:?}",
            staging_names(&root)
        );
    }

    /// The consume marker exists before the first write, so a run killed while
    /// writing still leaves a name `version` refuses on: refusing the marker
    /// must leave the write unattempted.
    #[test]
    fn the_consume_marker_is_claimed_before_any_write() {
        let root = scratch("marker-first");
        fs::create_dir_all(root.join(".changeset")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join(".changeset/gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Mark, ".changeset"), (Verb::Write, "a.txt")]);

        commit_write_set_under(
            &dir,
            &[PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1")],
            &[PlannedDelete::new(PathBuf::from(".changeset/gone.md"))],
            &faults,
        )
        .expect_err("the marker is refused");

        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A0");
        assert_eq!(
            faults.unclaimed(),
            [(Verb::Write, PathBuf::from("a.txt"))],
            "the write was never reached"
        );
    }

    /// A landed consume leaves neither its marker nor any set-aside file.
    #[test]
    fn a_landed_consume_removes_its_marker() {
        let root = scratch("marker-removed");
        fs::create_dir_all(root.join(".changeset")).unwrap();
        fs::write(root.join(".changeset/gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        commit_write_set(
            &dir,
            &[],
            &[PlannedDelete::new(PathBuf::from(".changeset/gone.md"))],
        )
        .expect("consumed")
        .into_result()
        .expect("nothing left");
        let left = staging_names(&root.join(".changeset"));
        assert!(left.is_empty(), "{left:?}");
    }

    /// Each kind of leftover that the next `version` must not plan over keeps
    /// the marker; one that is safe to remove does not. `Created` has no
    /// fault that reaches it, so this pins it directly.
    #[test]
    fn the_marker_stays_for_every_unsettled_leftover() {
        let path = || String::from("x");
        for entry in [
            LeftChanged::Unrestored {
                path: path(),
                err: path(),
            },
            LeftChanged::Created { path: path() },
            LeftChanged::Stranded {
                path: path(),
                staging: path(),
                err: path(),
            },
        ] {
            assert!(marker_must_stay(&[entry]));
        }
        assert!(!marker_must_stay(&[LeftChanged::StagingLeak {
            path: path()
        }]));
        assert!(!marker_must_stay(&[]));
    }

    /// The marker lists what the run consumes, which is how a later run tells
    /// a consume that never began from one that finished.
    #[test]
    fn the_consume_marker_lists_what_it_consumes() {
        let root = scratch("marker-body");
        fs::create_dir_all(root.join(".changeset")).unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let marker = create_consume_marker(
            &dir,
            Path::new(".changeset"),
            &[Path::new(".changeset/a.md"), Path::new(".changeset/b.md")],
        )
        .expect("marker");
        assert_eq!(
            fs::read_to_string(root.join(&marker)).unwrap(),
            ".changeset/a.md\n.changeset/b.md\n"
        );
    }

    /// A kept marker that cannot be marked as rolled back would be misread by
    /// the next `version`, so the rollback's own report says so.
    #[test]
    fn a_marker_that_cannot_be_marked_rolled_back_is_named() {
        let root = scratch("marker-unmarked");
        fs::create_dir_all(root.join(".changeset")).unwrap();
        fs::write(root.join(".changeset/one.md"), "O0").unwrap();
        fs::write(root.join(".changeset/two.md"), "T0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([
            (Verb::Stage, ".changeset/two.md"),
            (Verb::Restore, ".changeset/one.md"),
            (Verb::MarkRolledBack, ".changeset"),
        ]);

        let err = commit_write_set_under(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from(".changeset/one.md")),
                PlannedDelete::new(PathBuf::from(".changeset/two.md")),
            ],
            &faults,
        )
        .expect_err("the second stage is refused")
        .to_string();

        assert!(err.contains("could not be marked as rolled back"), "{err}");
        assert!(err.contains("remove no bump file"), "{err}");
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    /// A rollback that left a write unrestored keeps the marker, so the next
    /// `version` refuses rather than plan over the bumped manifest.
    #[test]
    fn a_rollback_that_left_a_write_keeps_the_marker() {
        let root = scratch("marker-kept");
        fs::create_dir_all(root.join(".changeset")).unwrap();
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join(".changeset/two.md"), "T0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults =
            Faults::refusing([(Verb::Stage, ".changeset/two.md"), (Verb::Restore, "a.txt")]);

        let err = commit_write_set_under(
            &dir,
            &[PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1")],
            &[PlannedDelete::new(PathBuf::from(".changeset/two.md"))],
            &faults,
        )
        .expect_err("the stage is refused")
        .to_string();

        let left = staging_names(&root.join(".changeset"));
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].starts_with(".version.oakum-consume."), "{left:?}");
        assert!(err.contains("kept: `oakum version` refuses"), "{err}");
        assert_eq!(
            fs::read_to_string(root.join(".changeset").join(&left[0])).unwrap(),
            "rolled-back\n",
            "the kept marker says no write is to be kept"
        );
        assert!(!err.contains(&format!("{} (an oakum", left[0])), "{err}");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A1");
    }

    /// At the repository root the stranded name and the leak sweep's name are
    /// spelled without a directory, and must still match.
    #[test]
    fn a_stranded_file_at_the_root_is_not_also_reported_as_a_leak() {
        let root = scratch("stage-restore-root");
        fs::write(root.join("keep.md"), "K0").unwrap();
        fs::write(root.join("gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Stage, "gone.md"), (Verb::Restore, "keep.md")]);

        let err = commit_write_set_under(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from("keep.md")),
                PlannedDelete::new(PathBuf::from("gone.md")),
            ],
            &faults,
        )
        .expect_err("the second stage is refused")
        .to_string();

        assert!(err.contains("keep.md (moved to ."), "{err}");
        assert!(!err.contains(STAGING_CLAIM), "{err}");
    }

    /// Once every write and stage landed the consume is complete, so a staged
    /// file that cannot be removed is returned to name and nothing rolls back.
    #[test]
    fn a_staged_file_that_cannot_be_removed_is_returned_and_the_writes_stay() {
        let root = scratch("remove-refused");
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Remove, "gone.md")]);

        let committed = commit_write_set_under(
            &dir,
            &[PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1")],
            &[PlannedDelete::new(PathBuf::from("gone.md"))],
            &faults,
        )
        .expect("the consume landed");

        let names = staging_names(&root);
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(fs::read_to_string(root.join(&names[0])).unwrap(), "G0");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A1");
        assert!(!root.join("gone.md").exists());
        let err = committed.into_result().expect_err("a leftover").to_string();
        assert!(
            err.contains(&format!(
                "\n  {} (refused by the test: Remove gone.md)",
                names[0]
            )),
            "{err}"
        );
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    #[test]
    fn a_landed_write_set_with_nothing_left_is_ok() {
        assert!(Committed {
            unremoved: Vec::new()
        }
        .into_result()
        .is_ok());
    }

    /// One fault strands every removal, and the names can arrive from a pull
    /// request: the list is capped and escaped like the failure report.
    #[test]
    fn leftovers_are_capped_and_escaped() {
        let unremoved = (0..HEAD_FILES + 2)
            .map(|n| {
                (
                    PathBuf::from(format!(".changeset/.e{n}\u{1b}[2J.md")),
                    String::from("x"),
                )
            })
            .collect();
        let text = Committed { unremoved }
            .into_result()
            .unwrap_err()
            .to_string();
        assert!(
            text.starts_with("every write landed and every bump file was consumed, but 7 file(s)"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(text.ends_with("\n  … 2 more"), "{text}");
    }

    /// The `done_writes` restore-failure push, on every platform: the landed
    /// write stays at `next` and is named.
    #[test]
    fn a_write_whose_restore_is_refused_is_named_and_stays_changed() {
        let root = scratch("write-restore-refused");
        fs::write(root.join("a.txt"), "A0").unwrap();
        fs::write(root.join("b.txt"), "B0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Write, "b.txt"), (Verb::Restore, "a.txt")]);

        let err = commit_write_set_under(
            &dir,
            &[
                PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1"),
                PlannedWrite::new(PathBuf::from("b.txt"), "B0", "B1"),
            ],
            &[],
            &faults,
        )
        .expect_err("the second write is refused")
        .to_string();

        assert!(err.contains("refused by the test: Write b.txt"), "{err}");
        assert!(err.contains("1 file(s) left changed:"), "{err}");
        assert!(
            err.contains("a.txt (restore failed: refused by the test: Restore a.txt)"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A1");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "B0");
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    /// A created file whose discard is refused is a leftover the sweep cannot
    /// see, so rollback names it itself.
    #[test]
    fn a_created_file_whose_discard_is_refused_is_named() {
        let root = scratch("discard-refused");
        fs::write(root.join("b.txt"), "B0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Write, "b.txt"), (Verb::Discard, "CHANGELOG.md")]);

        let err = commit_write_set_under(
            &dir,
            &[
                PlannedWrite::create(PathBuf::from("CHANGELOG.md"), "# Changelog\n"),
                PlannedWrite::new(PathBuf::from("b.txt"), "B0", "B1"),
            ],
            &[],
            &faults,
        )
        .expect_err("the second write is refused")
        .to_string();

        assert!(
            err.contains("CHANGELOG.md (restore failed: failed to remove CHANGELOG.md: refused by the test: Discard CHANGELOG.md)"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(root.join("CHANGELOG.md")).unwrap(),
            "# Changelog\n"
        );
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    /// Unscripted verbs reach the disk, and a refusal nothing needed stays
    /// unclaimed.
    #[test]
    fn an_unscripted_verb_reaches_the_filesystem() {
        let root = scratch("faults-none");
        fs::write(root.join("a.txt"), "A0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Restore, "a.txt")]);
        commit_write_set_under(
            &dir,
            &[PlannedWrite::new(PathBuf::from("a.txt"), "A0", "A1")],
            &[],
            &faults,
        )
        .expect("nothing refused the write itself")
        .into_result()
        .expect("no deletes, so nothing left");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A1");
        assert_eq!(
            faults.unclaimed(),
            [(Verb::Restore, PathBuf::from("a.txt"))],
            "a restore nothing needed stays scripted"
        );
    }

    /// Keyed by both verb and path, and claimed once: a second reach for the
    /// same key lands, as the git fake's answers do.
    #[test]
    fn a_scripted_refusal_is_claimed_once_by_verb_and_path() {
        let faults = Faults::refusing([(Verb::Write, "x")]);
        assert!(faults.refuse(Verb::Restore, Path::new("x")).is_none());
        assert!(faults.refuse(Verb::Write, Path::new("y")).is_none());
        assert!(faults.refuse(Verb::Write, Path::new("x")).is_some());
        assert!(faults.refuse(Verb::Write, Path::new("x")).is_none());
        assert!(faults.unclaimed().is_empty());
    }

    #[test]
    fn exclusive_create_does_not_replace_an_existing_file() {
        let root = scratch("create-exists");
        fs::write(root.join("CHANGELOG.md"), "user\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = commit_writes(
            &dir,
            &[PlannedWrite::create(
                PathBuf::from("CHANGELOG.md"),
                "# Changelog\n",
            )],
        )
        .expect_err("exists");
        assert!(err.to_string().contains("failed to create"), "{err}");
        assert_eq!(
            fs::read_to_string(root.join("CHANGELOG.md")).unwrap(),
            "user\n"
        );
    }
}
