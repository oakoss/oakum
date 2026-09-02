//! Restore already-landed files if a later write or delete fails.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;

use super::fs::{open_read_only, write_file_exclusive, write_file_via_rename};

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
                format!("{} is missing", path.display()),
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
                format!("failed to open {}: {err}", path.display()),
            )
            .into());
        }
    };
    let meta = file.metadata().map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to inspect {}: {err}", path.display()),
        )
    })?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        )
        .into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to read {}: {err}", path.display()),
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
    if let Some(path) = overlapping_path(writes, deletes) {
        return Err(format!(
            "write-set path appears in both writes and deletes: {}",
            path.display()
        )
        .into());
    }
    let mut done_writes = Vec::new();
    for write in writes {
        if write.original == write.next {
            continue;
        }
        let write_result: Result<(), Box<dyn std::error::Error>> = if write.created {
            write_file_exclusive(dir, &write.path, &write.next).map_err(Into::into)
        } else {
            write_file_via_rename(dir, &write.path, &write.next)
        };
        if let Err(err) = write_result {
            return Err(rollback(dir, &done_writes, &[], err.as_ref()));
        }
        done_writes.push(write);
    }
    let mut done_deletes = Vec::new();
    for delete in deletes {
        if let Err(err) = dir.remove_file(&delete.path) {
            return Err(rollback(
                dir,
                &done_writes,
                &done_deletes,
                &io_delete_err(&delete.path, &err),
            ));
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

fn io_delete_err(path: &Path, err: &std::io::Error) -> std::io::Error {
    std::io::Error::new(
        err.kind(),
        format!("failed to delete {}: {err}", path.display()),
    )
}

fn rollback(
    dir: &Dir,
    done_writes: &[&PlannedWrite],
    done_deletes: &[&PlannedDelete],
    err: &dyn std::error::Error,
) -> Box<dyn std::error::Error> {
    let mut message = err.to_string();
    for delete in done_deletes.iter().rev() {
        if let Err(restore_err) = write_file_via_rename(dir, &delete.path, &delete.original) {
            message = format!(
                "{message}; restoring {} also failed ({restore_err})",
                delete.path.display()
            );
        }
    }
    for write in done_writes.iter().rev() {
        let restore = if write.created {
            dir.remove_file(&write.path)
                .or_else(|err| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(err)
                    }
                })
                .map_err(|err| format!("failed to remove {}: {err}", write.path.display()))
        } else {
            write_file_via_rename(dir, &write.path, &write.original).map_err(|err| err.to_string())
        };
        if let Err(restore_err) = restore {
            message = format!(
                "{message}; restoring {} also failed ({restore_err})",
                write.path.display()
            );
        }
    }
    message.into()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use cap_std::fs::Dir;

    use crate::test_fixture::Fixture;

    use super::{commit_write_set, commit_writes, PlannedDelete, PlannedWrite, WriteSet};

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
        err.expect_err("third write");

        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A0");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "B0");
        assert_eq!(fs::read_to_string(root.join("c/file.txt")).unwrap(), "C0");
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
        err.expect_err("blocked delete");

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
        err.expect_err("second write");

        assert!(!root.join("CHANGELOG.md").exists());
        assert_eq!(fs::read_to_string(root.join("c/file.txt")).unwrap(), "C0");
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
