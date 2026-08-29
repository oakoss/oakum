//! A fixture directory that removes itself when the test ends, pass or fail.
//!
//! Unit tests reach this one. Integration tests carry their own copy in
//! `tests/support/fixture.rs`, which adds the git layer this module
//! deliberately omits: naming a process type here would fail
//! `tests/git_boundary.rs::only_the_git_module_spawns_a_process`, which holds
//! that nothing outside `cli/git` spawns a child. This module creates
//! directories and removes them, and that is all it may ever do.
//!
//! The root sits *inside* a container that the guard owns, so a test writing
//! beside its fixture — `root.parent().join(..)` — still writes into the
//! guarded tree.

use std::ffi::OsStr;
use std::io::Write as _;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Marks a container for `scripts/fixture-leak-check.sh`, which finds fixture
/// trees by structure rather than by name.
///
/// Deliberately not the integration guard's marker: that side writes a
/// `gitconfig` beside its marker and resolves one by finding it, so a shared
/// string would let a container from here answer a lookup from there and hand
/// back a path to a file nothing wrote.
pub(crate) const MARKER: &str = ".oakum-unit-fixture";

/// Where [`Drop`] records a container it could not remove: libtest captures
/// per-test stderr and prints it only for failures, so a leak in a green run
/// reaches nobody. A file survives the capture.
pub(crate) const LEDGER: &str = "fixture-leaks.log";

/// Set to anything but empty or `0` to keep fixtures for debugging. The
/// default deletes, so CI never accumulates; this is the opt-in that the old
/// remove-on-entry helpers gave unconditionally and undiscoverably.
const RETAIN: &str = "OAKUM_TEST_RETAIN";

fn base() -> PathBuf {
    match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir(),
    }
}

pub(crate) struct Fixture {
    container: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// A fresh container holding an empty `label` root.
    ///
    /// Uniqueness comes from the pid and a per-process counter rather than the
    /// label alone, so two tests may share a label without sharing a path.
    pub(crate) fn new(suite: &str, label: &str) -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        // Integration targets get `CARGO_TARGET_TMPDIR`; unit tests do not, and
        // fall back to the system temp directory.
        let container = base().join(format!(
            "oakum-{suite}-{label}-{}-{seq}",
            std::process::id()
        ));
        // `create_dir`, not `create_dir_all`: the name is fully determined by
        // the pid and the counter, so a run killed before `Drop` leaves a
        // container the next run at the same pid would otherwise adopt,
        // inheriting its contents as if they were fresh. Untested: the counter
        // is process-global, so a test cannot reserve the next name without
        // racing its neighbours.
        // Cargo creates the base only when it compiles a test target, so a
        // developer who removed it on the leak check's advice would otherwise
        // get `No such file or directory` from every fixture.
        std::fs::create_dir_all(base()).expect("fixture base");
        match std::fs::create_dir(&container) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => panic!(
                "fixture container {} already exists: a previous run leaked it, \
                 so remove that directory before rerunning",
                container.display()
            ),
            Err(err) => panic!("fixture container {}: {err}", container.display()),
        }
        let root = container.join(label);
        std::fs::create_dir_all(&root).expect("fixture root");
        std::fs::write(container.join(MARKER), "").expect("fixture marker");
        Self { container, root }
    }

    pub(crate) fn container(&self) -> &Path {
        &self.container
    }
}

impl Deref for Fixture {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

/// `Deref` does not satisfy an `AsRef<Path>` bound, and `fs::create_dir_all`,
/// `fs::metadata` and `Dir::open_ambient_dir` all take one.
impl AsRef<Path> for Fixture {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

impl AsRef<OsStr> for Fixture {
    fn as_ref(&self) -> &OsStr {
        self.root.as_os_str()
    }
}

impl Drop for Fixture {
    /// Three signals, because each survives a case the others do not: the
    /// panic needs a test that is not already failing and on the thread
    /// libtest is watching, the marker needs no process at all, and the ledger
    /// survives libtest's capture of a passing test's stderr.
    fn drop(&mut self) {
        if std::env::var_os(RETAIN).is_some_and(|value| !value.is_empty() && value != "0") {
            eprintln!("{RETAIN}: kept {}", self.container.display());
            return;
        }
        let err = match std::fs::remove_dir_all(&self.container) {
            Ok(()) => return,
            // Already gone is the outcome this wants, not a leak: a sweep, a
            // temp reaper, or someone acting on the gate's advice got here first.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => err,
        };
        // `read_dir` order is unspecified, so the marker may or may not have
        // survived the partial removal. Restoring is cheaper than checking.
        let restored = std::fs::write(self.container.join(MARKER), "");
        let recorded = record_leak(&self.container, &err);
        eprintln!("fixture leak: {} — {err}", self.container.display());
        if let Err(marker_err) = &restored {
            eprintln!("fixture marker not restored, so the gate is blind: {marker_err}");
        }
        if let Err(ledger_err) = &recorded {
            eprintln!("fixture ledger unwritable: {ledger_err}");
        }
        // Panicking while already unwinding aborts the process and takes the
        // test report with it, so this fires only on the green path.
        assert!(
            std::thread::panicking(),
            "fixture leak: {} — {err}",
            self.container.display()
        );
    }
}

/// One short `write_all`, not `writeln!`: `O_APPEND` makes an individual write
/// atomic, not a group of them, and the kernel does not split a line this
/// short. Measured, 32 concurrent appenders corrupted 6,157 of 6,400 lines.
fn record_leak(container: &Path, err: &std::io::Error) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(base().join(LEDGER))?
        .write_all(format!("{}\t{err}\n", container.display()).as_bytes())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{base, Fixture, LEDGER, MARKER};

    #[test]
    fn a_dropped_fixture_removes_its_container() {
        let container = {
            let root = Fixture::new("unit", "dropped");
            std::fs::write(root.join("file.txt"), "x").expect("write");
            assert!(root.container().join(MARKER).is_file());
            root.container().to_path_buf()
        };
        assert!(!container.exists(), "{} survived", container.display());
    }

    /// The property the old helpers never had, and the reason `Drop` may not
    /// panic: it runs while the test unwinds.
    #[test]
    fn a_panicking_test_still_removes_its_fixture() {
        let seen = std::sync::Mutex::new(std::path::PathBuf::new());
        let caught = std::panic::catch_unwind(|| {
            let root = Fixture::new("unit", "panicking");
            *seen.lock().expect("lock") = root.container().to_path_buf();
            panic!("the fixture must not outlive this panic");
        });
        assert!(caught.is_err(), "the probe was supposed to panic");
        let container = seen.lock().expect("lock").clone();
        assert!(!container.as_os_str().is_empty(), "never built one");
        assert!(!container.exists(), "{} survived", container.display());
    }

    /// All three routes, because each is defeatable alone: libtest swallows a
    /// passing test's stderr, `remove_dir_all` deletes the marker on its way
    /// to failing, and the ledger's own write can fail.
    #[cfg(unix)]
    #[test]
    fn a_container_that_cannot_be_removed_reaches_the_gate() {
        use std::os::unix::fs::PermissionsExt as _;

        let ledger_path = base().join(LEDGER);
        let seen = std::sync::Mutex::new(PathBuf::new());
        let blocked = std::sync::Mutex::new(PathBuf::new());
        let reclaimed = std::panic::catch_unwind(|| {
            let root = Fixture::new("unit", "unremovable");
            *seen.lock().expect("lock") = root.container().to_path_buf();
            // A directory without write permission cannot have its children
            // unlinked, which is what makes the reclaim fail.
            let stuck = root.join("stuck");
            std::fs::create_dir_all(stuck.join("child")).expect("stuck");
            std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o555))
                .expect("chmod");
            *blocked.lock().expect("lock") = stuck;
        });
        let container = seen.lock().expect("lock").clone();
        let recorded = std::fs::read_to_string(&ledger_path).unwrap_or_default();
        let marked = container.join(MARKER).is_file();
        // Before the assertions, so a regression strands no unwritable tree.
        // Removing the container is the whole repair: the gate ignores a ledger
        // entry whose container is gone, and rewriting that shared file would
        // race the other suites in this process appending to it.
        let _ = std::fs::set_permissions(
            &*blocked.lock().expect("lock"),
            std::fs::Permissions::from_mode(0o755),
        );
        std::fs::remove_dir_all(&container)
            .unwrap_or_else(|err| panic!("could not repair {}: {err}", container.display()));

        assert!(reclaimed.is_err(), "a failed reclaim must fail its test");
        assert!(
            marked,
            "{} kept no marker, so a lost ledger would hide it",
            container.display()
        );
        assert!(
            recorded.contains(&container.display().to_string()),
            "the failed reclaim was not recorded: {recorded:?}"
        );
    }

    /// A container something else removed first is the outcome the guard
    /// wants. Reporting it would redden a passing test and send its reader to
    /// a path that no longer exists.
    #[test]
    fn a_container_already_gone_is_not_reported() {
        let ledger_path = base().join(LEDGER);
        let container = {
            let root = Fixture::new("unit", "vanished");
            let container = root.container().to_path_buf();
            std::fs::remove_dir_all(&container).expect("remove ahead of Drop");
            container
        };
        let recorded = std::fs::read_to_string(&ledger_path).unwrap_or_default();
        assert!(
            !recorded.contains(&container.display().to_string()),
            "an already-gone container was reported as a leak: {recorded:?}"
        );
    }

    /// What retires the label registry: a shared label is no longer a shared
    /// path, so one test cannot delete another's fixture mid-run.
    #[test]
    fn the_same_label_twice_gets_two_containers() {
        let one = Fixture::new("unit", "same");
        let two = Fixture::new("unit", "same");
        assert_ne!(one.container(), two.container());
        assert!(one.is_dir() && two.is_dir());
    }
}
