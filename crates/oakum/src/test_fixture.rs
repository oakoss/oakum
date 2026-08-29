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
//!
//! Unused until the unit-test suites convert; the tests at the foot of this
//! file are what exercise it meanwhile.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::Write as _;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Marks a container for `.mise.toml`'s sweep, which finds fixture trees by
/// structure rather than by name.
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
    /// Errors are swallowed rather than unwrapped: a panic here during a
    /// failing test would abort the process while it is already unwinding,
    /// and the test report would be lost along with the failure it was
    /// reporting.
    fn drop(&mut self) {
        if std::env::var_os(RETAIN).is_some_and(|value| !value.is_empty() && value != "0") {
            eprintln!("{RETAIN}: kept {}", self.container.display());
            return;
        }
        // Not panicking and not reporting are separate decisions, and only
        // the first is forced: a write is safe while unwinding. Without the
        // line, a container that cannot be removed leaks in a green, silent
        // run — the failure this guard exists to prevent.
        let Err(err) = std::fs::remove_dir_all(&self.container) else {
            return;
        };
        eprintln!("fixture leak: {} — {err}", self.container.display());
        if let Ok(mut ledger) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(base().join(LEDGER))
        {
            // One `write_all`, not `writeln!`: the latter issues a write per
            // format fragment, and `O_APPEND` makes each write atomic rather
            // than the group. Measured, 32 concurrent appenders corrupted
            // 6,157 of 6,400 lines.
            let line = format!("{}\t{err}\n", self.container.display());
            let _ = ledger.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Fixture, MARKER};

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
