//! Restore already-landed files if a later write fails.

use std::path::{Path, PathBuf};

use cap_std::fs::Dir;

use super::config::write_file_via_rename;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlannedWrite {
    path: PathBuf,
    original: String,
    next: String,
}

impl PlannedWrite {
    pub(super) fn new(path: PathBuf, original: impl Into<String>, next: impl Into<String>) -> Self {
        Self {
            path,
            original: original.into(),
            next: next.into(),
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
}

/// # Errors
///
/// Already-landed files are restored to `original` before the error is returned.
pub(super) fn commit_writes(
    dir: &Dir,
    writes: &[PlannedWrite],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut done = Vec::new();
    for write in writes {
        if write.original == write.next {
            continue;
        }
        if let Err(err) = write_file_via_rename(dir, &write.path, &write.next) {
            return Err(rollback(dir, &done, err.as_ref()));
        }
        done.push(write);
    }
    Ok(())
}

fn rollback(
    dir: &Dir,
    done: &[&PlannedWrite],
    err: &dyn std::error::Error,
) -> Box<dyn std::error::Error> {
    let mut message = err.to_string();
    for write in done.iter().rev() {
        if let Err(restore_err) = write_file_via_rename(dir, &write.path, &write.original) {
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
    use std::path::PathBuf;

    use cap_std::fs::Dir;

    use super::{commit_writes, PlannedWrite};

    fn scratch(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("oakum-write-set-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
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
}
