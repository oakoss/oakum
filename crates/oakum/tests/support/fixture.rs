//! Fixture repositories that clean up after themselves and read a git config
//! this suite owns.
//!
//! Each integration test is its own crate and compiles this module whole, so a
//! helper only some of them call is dead code in the rest — the same reason
//! [`super`] carries its own allow. `disallowed_methods` is allowed as every
//! test crate allows it: `tests/` is outside ADR-0002's boundary.
//!
//! The guard duplicates `src/test_fixture.rs` rather than reaching into it:
//! two definitions replace the twenty per-file helpers this module retires,
//! and a `#[path]` climb out of `tests/` into `src/` buys one definition at the
//! cost of coupling the two trees.
#![allow(clippy::disallowed_methods, dead_code)]

use std::ffi::OsStr;
use std::io::Write as _;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// Marks a container for `scripts/fixture-leak-check.sh`, which finds fixture
/// trees by structure rather than by name: a name glob would also match
/// `oakum-changeset-foreign`, the deliberately cached `pnpm install`.
///
/// Distinct from the unit guard's marker. Only this side writes a `gitconfig`,
/// and [`sandbox_config`] treats a marker as the promise that one sits beside
/// it, so a shared string would let a unit container answer an integration
/// lookup with a path to a file nothing wrote.
pub const MARKER: &str = ".oakum-fixture";

const RETAIN: &str = "OAKUM_TEST_RETAIN";

/// Where [`Drop`] records a container it could not remove.
///
/// The `eprintln!` beside it reaches nobody on a passing test: libtest captures
/// per-test stderr and prints it only for failures, so a leak in a green run is
/// silent (measured — an undeletable fixture reported `1 passed` and nothing
/// else). A file survives the capture, and the leak check reads it.
pub const LEDGER: &str = "fixture-leaks.log";

/// What a fixture must supply, or defend against. Deliberately small: only
/// these two classes survive measurement.
///
/// **Supplied**, because git has no usable default — `user.*`, without which a
/// commit fails, and `init.defaultBranch`, which git 2.55 answers with
/// `master` and a warning while this suite assumes `main`.
///
/// **Defensive**, because git reads these two files *outside* the config tier,
/// so replacing the tier does not stop them: measured, a `~/.config/git/ignore`
/// carrying `*.log` hides a fixture's `noisy.log` from `status
/// --untracked-files=all` until `excludesFile` is pinned empty.
///
/// Nothing else belongs here. `commit.gpgSign`, `push.followTags`,
/// `core.autocrlf`, `fetch.all` and the rest of a developer's config never
/// arrive: `GIT_CONFIG_GLOBAL` replaces the whole global tier and
/// `GIT_CONFIG_NOSYSTEM` disables the system one (measured — `config
/// --show-origin push.followTags` finds nothing against an ambient `true`). A
/// pin that defends against nothing still costs: it pre-satisfies conditions
/// production code is responsible for handling, so a regression dropping the
/// real handling would still pass.
const SEED: &str = "\
[user]
\tname = oakum test
\temail = oakum@test.invalid
[init]
\tdefaultBranch = main
[core]
\texcludesFile = \n\
\tattributesFile = \n\
";

fn base() -> PathBuf {
    match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        // No `CARGO_TARGET_TMPDIR` in unit tests: use `OAKUM_TARGET_TMP` from
        // `build.rs` (leak-check path). `option_env`: integration skips the env.
        None => {
            if let Some(dir) = option_env!("OAKUM_TARGET_TMP") {
                PathBuf::from(dir)
            } else {
                panic!("unit fixtures need OAKUM_TARGET_TMP from crates/oakum/build.rs");
            }
        }
    }
}

/// A fixture root inside a container the guard removes on drop.
///
/// Derefs to the root, so `let root = git_repo(..)` reads as it always did.
/// `root.parent()` is the container, so a bare origin, a PATH shim or a clone
/// written beside the root lands inside the guarded tree and resolves the same
/// sandboxed git config.
#[derive(Debug)]
pub struct Fixture {
    container: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(suite: &str, label: &str) -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let container = base().join(format!(
            "oakum-{suite}-{label}-{}-{seq}",
            std::process::id()
        ));
        // `create_dir`, not `create_dir_all`: every component of the name is
        // determined by the pid and the counter, so a run killed before `Drop`
        // leaves a container the next run at the same pid would otherwise
        // adopt — inheriting its `.git`, its tags and its edited config as if
        // they were fresh. Untested: the counter is process-global, so a test
        // cannot reserve the name the next call will take without racing the
        // other tests in the binary.
        // Cargo creates the base only when it compiles a test target, so a
        // developer who removed it on the leak check's advice would otherwise
        // get `No such file or directory` from every fixture.
        std::fs::create_dir_all(base()).expect("fixture base");
        match std::fs::create_dir(&container) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => panic!(
                "fixture container {} already exists: a previous run leaked it, \
                 so remove it (or `cargo clean`) before rerunning",
                container.display()
            ),
            Err(err) => panic!("fixture container {}: {err}", container.display()),
        }
        let root = container.join(label);
        std::fs::create_dir_all(&root).expect("fixture root");
        std::fs::write(container.join("gitconfig"), SEED).expect("sandbox gitconfig");
        std::fs::write(container.join(MARKER), "").expect("fixture marker");
        Self { container, root }
    }

    pub fn container(&self) -> &Path {
        &self.container
    }
}

impl Deref for Fixture {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

/// `Deref` does not satisfy an `AsRef<Path>` bound, and `Command::current_dir`,
/// `fs::create_dir_all`, `fs::metadata` and `Dir::open_ambient_dir` all take
/// one.
impl AsRef<Path> for Fixture {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

/// `Command::arg` takes `AsRef<OsStr>`, which the `Path` impl does not cover.
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

/// A fixture whose root is an initialized repository on `main`.
pub fn git_repo(suite: &str, label: &str) -> Fixture {
    let fixture = Fixture::new(suite, label);
    git(&fixture, &["init"]);
    fixture
}

/// A fixture whose root is an empty directory: for the suites that assert on a
/// missing repository, or fake one with a bare `.git` directory.
///
/// [`git_env`]'s ceiling is what makes the first use honest — containers sit
/// under `target/`, inside this checkout, so without it a discovery walk that
/// leaves the fixture finds the oakum repository and answers about that.
pub fn plain_repo(suite: &str, label: &str) -> Fixture {
    Fixture::new(suite, label)
}

/// A path beside the fixture root, still inside the container `Drop` removes.
///
/// # Panics
///
/// When `name` is not a single normal path segment, or resolves to the fixture
/// root itself (`"label/"` still lands on the root).
pub fn sibling(root: &Fixture, name: &str) -> PathBuf {
    let mut parts = Path::new(name).components();
    assert!(
        matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none(),
        "sibling name must be one path segment inside the container, got {name:?}"
    );
    let path = root.container().join(name);
    // `"label/"` joins to the same path as the fixture root and would dirty it.
    assert!(
        path.as_path() != AsRef::<Path>::as_ref(root),
        "sibling name must not equal the fixture label (repo root), got {name:?}"
    );
    path
}

/// The fixture-owned global config governing `inside`.
///
/// Walks ancestors rather than taking `parent()`, so it answers for the root,
/// for a sibling written beside it, and for a path nested under either.
///
/// # Panics
///
/// When `inside` is not within a fixture container, or the container carries no
/// `gitconfig`. Measured on git 2.55, only `config --list` refuses a missing
/// config file: `commit` exits 0 and writes the developer's gecos identity, so
/// an absent config would quietly unpin everything rather than fail.
pub fn sandbox_config(inside: &Path) -> PathBuf {
    for dir in inside.ancestors() {
        if dir.join(MARKER).is_file() {
            let config = dir.join("gitconfig");
            assert!(
                config.is_file(),
                "{} is marked as a fixture container but has no gitconfig, so \
                 git would run unisolated and commit as the developer",
                dir.display()
            );
            return config;
        }
    }
    panic!(
        "{} is not inside a fixture container, so it has no sandboxed git config",
        inside.display()
    )
}

/// The isolation every child in the suite carries, `git` and the oakum binary
/// alike.
///
/// Clears the whole `GIT_*` namespace rather than naming variables to remove.
/// Git's environment outranks every config tier, the namespace has around forty
/// members, and chasing them one at a time loses: measured, `GIT_ALLOW_PROTOCOL`
/// overrides even an explicit `-c protocol.allow=never`, `GIT_NAMESPACE` makes
/// `ls-remote` answer nothing at exit 0, and the pathspec variables empty an
/// `ls-files` result the same way. A caller that wants one of them sets it
/// *after* this returns.
///
/// `GIT_CONFIG_GLOBAL` names a file rather than `/dev/null`: production reads
/// the user's `core.sshCommand`, `ssh.variant` and `remote.*.tagopt` from the
/// global tier, so a suite that removes the tier tests a configuration the tool
/// never runs in.
pub fn git_env<'a>(command: &'a mut Command, inside: &Path) -> &'a mut Command {
    // The clear reaches the parent's environment, not the command's, so a
    // caller that sets its own `GIT_*` first is silently dropped on a machine
    // that exports the same name and honoured on one that does not. Refusing
    // makes that ordering a compile-time-ish error rather than a machine-
    // dependent surprise.
    assert!(
        !command
            .get_envs()
            .any(|(key, _)| key.to_string_lossy().starts_with("GIT_")),
        "git_env clears the GIT_* namespace, so it must run before any GIT_* \
         override of your own"
    );
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(&key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", sandbox_config(inside))
        .env("GIT_TERMINAL_PROMPT", "0")
        // `EDITOR` and `VISUAL` are not `GIT_*`-prefixed, so the sweep above
        // does not reach them and git falls back to whichever the developer
        // exports (measured: `git var GIT_EDITOR` answered with `EDITOR`, then
        // with `VISUAL`). Nothing opens an editor today — every write passes
        // `-m` — but the reasoning that sets `GIT_TERMINAL_PROMPT=0` applies.
        .env("GIT_EDITOR", "false")
        // The system attributes file has its own switch: `GIT_CONFIG_NOSYSTEM`
        // covers system *config* only, and the seed's empty `attributesFile`
        // replaces the *global* file. Inferred, not measured — planting one at
        // git's compiled prefix needs a writable `/opt/homebrew`.
        .env("GIT_ATTR_NOSYSTEM", "1")
        // Containers sit under `target/`, inside this checkout, so without a
        // ceiling any repository-discovery walk that leaves a fixture finds the
        // oakum repository and reports on it instead.
        .env("GIT_CEILING_DIRECTORIES", base())
}

/// Runs git in `inside`, asserting it succeeded.
///
/// # Panics
///
/// When git cannot be spawned, or exits non-zero.
pub fn git(inside: &Path, args: &[&str]) {
    let output = git_output(inside, args);
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        inside.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Trimmed stdout of a git read, asserting it succeeded.
///
/// The default for reads: a failed git yields empty stdout, which a caller
/// comparing against an empty expected value, asserting `is_empty()`, or
/// negating a `contains`, reads as an answer. [`git_output`] is for the callers
/// that assert on a failure.
///
/// # Panics
///
/// When git cannot be spawned, or exits non-zero.
pub fn git_stdout(inside: &Path, args: &[&str]) -> String {
    let output = git_output(inside, args);
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        inside.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Runs git in `inside` and hands back the result unjudged, for callers that
/// assert on a failure.
///
/// # Panics
///
/// When git cannot be spawned.
pub fn git_output(inside: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    git_env(&mut command, inside)
        .args(args)
        .current_dir(inside)
        .output()
        .expect("git")
}

/// The oakum binary, pointed at `inside` and carrying the same isolation.
pub fn oakum(inside: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oakum"));
    git_env(&mut command, inside).current_dir(inside);
    command
}
