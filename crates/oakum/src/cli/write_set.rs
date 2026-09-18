//! Restore already-landed files if a later write or delete fails.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;

use cap_std::fs::Dir;

use super::fs::{
    open_read_only, own_staging_files, repo_path_display, write_file_exclusive,
    write_file_via_rename, STAGING_CLAIM,
};

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
    original: String,
}

impl PlannedDelete {
    pub(super) fn new(path: PathBuf, original: impl Into<String>) -> Self {
        Self {
            path,
            original: original.into(),
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// # Errors
///
/// Already-landed files are restored to `original` before the error is returned.
#[cfg(test)]
pub(super) fn commit_writes(
    dir: &Dir,
    writes: &[PlannedWrite],
) -> Result<(), Box<dyn std::error::Error>> {
    commit_write_set(dir, writes, &[])
}

/// One of the filesystem verbs a write set performs, forward or in rollback.
/// Rollback verbs are their own so a test can let a write land and refuse
/// only its restore.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Create,
    Write,
    Remove,
    /// Writing `original` back over a landed write or delete.
    Restore,
    /// Removing a file this run created.
    Discard,
}

/// Refusals a test scripts, keyed by verb and path the way the git fake keys
/// answers by operation: each is claimed once, everything unscripted reaches
/// the real filesystem, and an unclaimed refusal is one the code was right
/// not to reach — [`Self::unclaimed`] says which. The shipping path carries
/// none. A refused verb leaves the disk as the fault it stands in for would:
/// the delete-side `Restore` has a real driver (a name near `NAME_MAX`,
/// tested below) and the disk states match; `Discard` and the write-side
/// `Restore` need one filesystem permission for the landing and another for
/// the undoing, which no runner's flags express mid-run.
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

/// A later failure restores completed deletes, then writes.
///
/// # Errors
///
/// Already-landed files are restored to `original` before the error is returned.
pub(super) fn commit_write_set(
    dir: &Dir,
    writes: &[PlannedWrite],
    deletes: &[PlannedDelete],
) -> Result<(), Box<dyn std::error::Error>> {
    commit_write_set_under(dir, writes, deletes, &Faults::none())
}

fn commit_write_set_under(
    dir: &Dir,
    writes: &[PlannedWrite],
    deletes: &[PlannedDelete],
    faults: &Faults,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = overlapping_path(writes, deletes) {
        return Err(format!(
            "write-set path appears in both writes and deletes: {}",
            repo_path_display(path)
        )
        .into());
    }
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
            return Err(Box::new(rollback(
                dir,
                &done_writes,
                &[],
                Some(attempt),
                err.as_ref(),
                faults,
            )));
        }
        done_writes.push(write);
    }
    let mut done_deletes = Vec::new();
    for delete in deletes {
        let removed = faults.attempt(Verb::Remove, &delete.path, || dir.remove_file(&delete.path));
        if let Err(err) = removed {
            return Err(Box::new(rollback(
                dir,
                &done_writes,
                &done_deletes,
                Some(Attempt::Delete(&delete.path)),
                &io_delete_err(&delete.path, &err),
                faults,
            )));
        }
        done_deletes.push(delete);
    }
    Ok(())
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

fn io_delete_err(path: &Path, err: &std::io::Error) -> std::io::Error {
    std::io::Error::new(
        err.kind(),
        format!("failed to delete {}: {err}", repo_path_display(path)),
    )
}

/// One file the tree kept, and which way. Rendering stays here so a caller can
/// ask what was left without parsing the sentence it prints.
#[derive(Debug)]
pub(super) enum LeftChanged {
    /// A restore that reported failure: the file holds neither its original
    /// nor the planned content reliably.
    Unrestored { path: String, err: String },
    /// A create that landed and whose removal failed.
    Created { path: String },
    /// A staging file this run wrote and could not remove.
    StagingLeak { path: String },
}

impl fmt::Display for LeftChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrestored { path, err } => write!(f, "{path} (restore failed: {err})"),
            Self::Created { path } => write!(f, "{path} (created and could not be removed)"),
            Self::StagingLeak { path } => write!(f, "{path} ({STAGING_CLAIM})"),
        }
    }
}

/// A write set that failed partway, and everything it could not put back.
///
/// An empty `left_changed` means every restore reported success and every
/// swept directory could be read — weaker than a byte-identical tree, because
/// rollback writes `original` back without re-reading to confirm it. File
/// identity, mode, and a plan gone stale since the read are outside the claim.
#[derive(Debug)]
pub(super) struct WriteSetFailure {
    cause: String,
    left_changed: Vec<LeftChanged>,
    unswept: Vec<String>,
}

impl WriteSetFailure {
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

impl fmt::Display for WriteSetFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.cause)?;
        if !self.left_changed.is_empty() {
            write!(f, "\n{} file(s) left changed:", self.left_changed.len())?;
            for entry in &self.left_changed {
                write!(f, "\n  {entry}")?;
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
                write!(f, "\n  {entry}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for WriteSetFailure {}

fn rollback(
    dir: &Dir,
    done_writes: &[&PlannedWrite],
    done_deletes: &[&PlannedDelete],
    attempted: Option<Attempt<'_>>,
    err: &dyn std::error::Error,
    faults: &Faults,
) -> WriteSetFailure {
    let cause = err.to_string();
    let mut left_changed = Vec::new();
    for delete in done_deletes.iter().rev() {
        let restored = faults.attempt(Verb::Restore, &delete.path, || {
            write_file_via_rename(dir, &delete.path, &delete.original)
        });
        if let Err(restore_err) = restored {
            left_changed.push(LeftChanged::Unrestored {
                path: repo_path_display(&delete.path),
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
    let (leaked, unswept) = own_staging_leftovers(dir, done_writes, done_deletes, attempted);
    left_changed.extend(leaked);
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
    done_deletes: &[&PlannedDelete],
    attempted: Option<Attempt<'_>>,
) -> (Vec<LeftChanged>, Vec<String>) {
    let subs: BTreeSet<String> = done_writes
        .iter()
        .map(|write| write.path.as_path())
        .chain(done_deletes.iter().map(|delete| delete.path.as_path()))
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

    use super::{
        commit_write_set, commit_write_set_under, commit_writes, Faults, LeftChanged,
        PlannedDelete, PlannedWrite, Verb, WriteSet, WriteSetFailure,
    };
    // Only the staging-sweep tests read it, and those are unix-only.
    #[cfg(unix)]
    use super::STAGING_CLAIM;

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
        // A second leak in the directory of a delete that SUCCEEDED, which only
        // `done_deletes` reaches: rollback restores through the same writer.
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
                PlannedDelete::new(PathBuf::from("done/ok.md"), "O0"),
                PlannedDelete::new(PathBuf::from("gone/second.md"), "S0"),
            ],
        );
        let mut restore = fs::metadata(&gone).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&gone, restore).unwrap();

        let err = expect_refused(result, "blocked delete").to_string();
        assert!(err.contains("failed to delete gone/second.md"), "{err}");
        for named in [&leaked, &restored_leak, &nested] {
            assert!(err.contains(&format!("{named} ({STAGING_CLAIM})")), "{err}");
        }
    }

    #[test]
    fn a_created_file_with_an_empty_body_is_still_written() {
        let root = scratch("create-empty");
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        // `create` leaves `original` empty, so the unchanged-text skip would
        // drop this write and report success over a file that never appeared.
        commit_write_set(
            &dir,
            &[PlannedWrite::create(PathBuf::from("empty.txt"), "")],
            &[],
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

    #[test]
    fn overlapping_write_and_delete_is_an_error() {
        let root = scratch("overlap");
        fs::write(root.join("same.txt"), "old").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = commit_write_set(
            &dir,
            &[PlannedWrite::new(PathBuf::from("same.txt"), "old", "new")],
            &[PlannedDelete::new(PathBuf::from("same.txt"), "old")],
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
                PlannedDelete::new(PathBuf::from("keep.md"), "K0"),
                PlannedDelete::new(PathBuf::from("blocked/gone.md"), "G0"),
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

    /// The `done_deletes` restore-failure push: a delete that landed and a
    /// restore that did not leaves the file gone, and the report says so.
    #[test]
    fn a_delete_whose_restore_is_refused_is_named_and_stays_gone() {
        let root = scratch("delete-restore-refused");
        fs::write(root.join("keep.md"), "K0").unwrap();
        fs::write(root.join("gone.md"), "G0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let faults = Faults::refusing([(Verb::Remove, "gone.md"), (Verb::Restore, "keep.md")]);

        let err = commit_write_set_under(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from("keep.md"), "K0"),
                PlannedDelete::new(PathBuf::from("gone.md"), "G0"),
            ],
            &faults,
        )
        .expect_err("the second delete is refused")
        .to_string();

        assert!(err.contains("failed to delete gone.md"), "{err}");
        assert!(err.contains("1 file(s) left changed:"), "{err}");
        assert!(
            err.contains("keep.md (restore failed: refused by the test: Restore keep.md)"),
            "{err}"
        );
        assert!(
            !root.join("keep.md").exists(),
            "the refused restore left it gone"
        );
        assert_eq!(fs::read_to_string(root.join("gone.md")).unwrap(), "G0");
        assert!(faults.unclaimed().is_empty(), "{:?}", faults.unclaimed());
    }

    /// The one restore failure a real fault reaches on its own: rollback
    /// restores a delete through `write_file_via_rename`, whose staging name
    /// is about thirty bytes longer than the target's, so a name near
    /// `NAME_MAX` deletes and cannot be put back. The report and the disk
    /// state match the scripted refusal above, which is what lets the script
    /// stand in for the fault. Unix only: the staging name's overhead was
    /// measured against this filesystem's 255-byte `NAME_MAX` (the restore
    /// first fails at 225), and no equivalent was established on Windows.
    #[cfg(unix)]
    #[test]
    fn a_delete_whose_restore_outruns_name_max_is_named_and_stays_gone() {
        let root = scratch("delete-restore-long-name");
        let long = "l".repeat(240);
        fs::write(root.join(&long), "L0").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();

        let err = commit_write_set(
            &dir,
            &[],
            &[
                PlannedDelete::new(PathBuf::from(&long), "L0"),
                PlannedDelete::new(PathBuf::from("absent.md"), "A0"),
            ],
        )
        .expect_err("the second delete fails")
        .to_string();

        assert!(err.contains("failed to delete absent.md"), "{err}");
        assert!(err.contains("1 file(s) left changed:"), "{err}");
        assert!(
            err.contains(&format!("{long} (restore failed: failed to stage `{long}`")),
            "{err}"
        );
        assert!(
            !root.join(&long).exists(),
            "the restore could not stage, so the file stays gone"
        );
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
        .expect("nothing refused the write itself");
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
