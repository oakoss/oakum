//! Fixture repositories that clean up after themselves and read a git config
//! this suite owns.
//!
//! The guard duplicates `src/test_fixture.rs` rather than reaching into it:
//! two definitions replace the twenty per-file helpers this module retires,
//! and a `#[path]` climb out of `tests/` into `src/` buys one definition at the
//! cost of coupling the two trees. The leak diagnosis is the exception: it
//! touches nothing either side owns, so both include one file.

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
/// these three classes survive measurement.
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
/// **Quiet**, because git's automatic maintenance outlives the command that
/// starts it: measured, every fixture commit, fetch, merge and push starts a
/// detached `git maintenance run --auto`, still running when [`Drop`] removes
/// the container. It is a suspected writer behind okm-mru6's late
/// `.git/objects/info/packs`, not a confirmed one. oakum never runs or depends
/// on maintenance, so the pin hides nothing production handles.
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
[maintenance]
\tauto = false
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
    #[cfg(unix)]
    pub(super) hermetic_bin: std::sync::OnceLock<PathBuf>,
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
        Self {
            container,
            root,
            #[cfg(unix)]
            hermetic_bin: std::sync::OnceLock::new(),
        }
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
        // Before the marker goes back, so the listing shows only what was left.
        let diagnosis = leak_diagnosis(&self.container);
        // `read_dir` order is unspecified, so the marker may or may not have
        // survived the partial removal. Restoring is cheaper than checking.
        let restored = std::fs::write(self.container.join(MARKER), "");
        let recorded = record_leak(&self.container, &format!("{err} — {diagnosis}"));
        eprintln!(
            "fixture leak: {} — {err} — {diagnosis}",
            self.container.display()
        );
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
            "fixture leak: {} — {err} — {diagnosis}",
            self.container.display()
        );
    }
}

/// One `write_all`, not `writeln!`: `O_APPEND` makes an individual write
/// atomic, not a group of them. Measured, 32 concurrent appenders corrupted
/// 6,157 of 6,400 lines.
fn record_leak(container: &Path, reason: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(base().join(LEDGER))?
        .write_all(format!("{}\t{reason}\n", container.display()).as_bytes())
}

#[path = "../../../src/test_fixture/leak_diagnosis.rs"]
mod leak_diagnosis;
pub(crate) use leak_diagnosis::leak_diagnosis;

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

/// A [`plain_repo`] with an empty `.git`: enough for oakum to find a root,
/// while every git command fails.
pub fn fake_git_dir_repo(suite: &str, label: &str) -> Fixture {
    let root = plain_repo(suite, label);
    std::fs::create_dir(root.join(".git")).expect("fixture .git");
    root
}

/// A bare repository beside `root`, added as its `origin`.
pub fn add_bare_origin(root: &Fixture) -> PathBuf {
    let remote = sibling(root, "origin.git");
    let remote_arg = remote.to_str().expect("utf-8");
    git(root, &["init", "--bare", remote_arg]);
    git(root, &["remote", "add", "origin", remote_arg]);
    remote
}

pub fn head_sha(root: &Path) -> String {
    git_stdout(root, &["rev-parse", "HEAD"])
}

/// A directory held at `mode` until drop puts `0o755` back. As a guard, so a
/// panic in between cannot leave entries `Fixture`'s drop is denied removing.
#[cfg(unix)]
pub struct Sealed<'a> {
    dir: &'a Path,
}

#[cfg(unix)]
impl<'a> Sealed<'a> {
    pub fn set(dir: &'a Path, mode: u32) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))
            .unwrap_or_else(|err| panic!("chmod {mode:o} {}: {err}", dir.display()));
        Self { dir }
    }
}

#[cfg(unix)]
impl Drop for Sealed<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(self.dir, std::fs::Permissions::from_mode(0o755));
    }
}

/// A file `chflags uchg` makes immutable until drop clears it, for the same
/// reason as [`Sealed`]. macOS-only: Linux's `chattr +i` needs
/// `CAP_LINUX_IMMUTABLE`, which CI does not have.
#[cfg(target_os = "macos")]
pub struct Immutable<'a> {
    root: &'a Path,
    relative: &'a str,
}

#[cfg(target_os = "macos")]
impl<'a> Immutable<'a> {
    pub fn set(root: &'a Path, relative: &'a str) -> Self {
        assert!(chflags(root, "uchg", relative), "chflags uchg {relative}");
        Self { root, relative }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Immutable<'_> {
    fn drop(&mut self) {
        chflags(self.root, "nouchg", self.relative);
    }
}

#[cfg(target_os = "macos")]
fn chflags(root: &Path, flag: &str, relative: &str) -> bool {
    Command::new("chflags")
        .args([flag, relative])
        .current_dir(root)
        .status()
        .is_ok_and(|status| status.success())
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
///
/// `GITHUB_API_URL` defaults to a port nothing listens on so a test that forgets
/// its mock server fails on a refused connection rather than reaching
/// `api.github.com`. A live call passes until it fails for a reason the suite
/// does not control — a rate limit, an outage — which surfaces as an unrelated
/// test refusing long after the test was written. A test that wants a server
/// overrides it with its own `.env`.
pub fn oakum(inside: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oakum"));
    git_env(&mut command, inside)
        .env("GITHUB_API_URL", "http://127.0.0.1:1")
        .current_dir(inside);
    command
}

pub fn cargo_package(root: &Path, name: &str, version: &str) {
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[workspace]\n"
        ),
    )
    .expect("Cargo.toml");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), "").expect("lib.rs");
}

/// One member of a private Cargo workspace: a package a registry refuses, which
/// is what makes `private-packages` the term that decides whether oakum manages
/// it.
pub fn private_member(root: &Path, name: &str, version: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("src")).expect("member src");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\npublish = false\n"
        ),
    )
    .expect("member Cargo.toml");
    std::fs::write(dir.join("src/lib.rs"), "").expect("member lib.rs");
}

/// A workspace whose members are all unpublishable, so `private-packages.tag`
/// is the deciding term in taggability — the shape no other fixture carries.
/// `release_cli`'s `write_workspace` is the publishable sibling; the
/// `publish = false` line is the whole difference, and it is the line these
/// tests turn on.
pub fn private_workspace(root: &Path, members: &[(&str, &str)]) {
    let names = members
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{names}]\n"),
    )
    .expect("workspace Cargo.toml");
    for (name, version) in members {
        private_member(root, name, version);
    }
}

/// The tag each member already carries, in the shape changesets and bumpy
/// monorepos write: `<name>@<version>`, where oakum's multi-package default
/// renders `<name>/v<version>`.
pub fn tag_members_at_version(root: &Path, members: &[(&str, &str)]) {
    for (name, version) in members {
        let tag = format!("{name}@{version}");
        git(root, &["tag", "-a", &tag, "-m", &tag]);
    }
}

/// oakoss/tm's root manifest: `tm-store` inherits `gitdep` and `rusqlite`, and
/// `tempfile` only as a dev-dependency.
pub const TM_ROOT_MANIFEST: &str = r#"[workspace]
resolver = "2"
members = ["tm-cli", "tm-core", "tm-store"]

[workspace.package]
edition = "2021"

[workspace.dependencies]
gitdep = { git = "https://example.invalid/gitdep.git", branch = "main" }
rusqlite = { version = "0.31", features = ["bundled"] }
tempfile = "3.10"

[workspace.metadata.tm]
note = "one"
"#;

/// Hand-written, so no test resolves anything; `cargo metadata --no-deps`
/// never reads it.
pub const TM_LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "fastrand"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"

[[package]]
name = "gitdep"
version = "0.1.0"
source = "git+https://example.invalid/gitdep.git?branch=main#aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[package]]
name = "libsqlite3-sys"
version = "0.28.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222222222222222222222222222222222222222222222222222222222222222"

[[package]]
name = "rusqlite"
version = "0.31.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333333333333333333333333333333333333333333333333333333333333333"
dependencies = [
 "libsqlite3-sys",
]

[[package]]
name = "tempfile"
version = "3.10.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4444444444444444444444444444444444444444444444444444444444444444"
dependencies = [
 "fastrand",
]

[[package]]
name = "tm-cli"
version = "0.1.0"
dependencies = [
 "tm-core",
]

[[package]]
name = "tm-core"
version = "0.1.0"
dependencies = [
 "tm-store",
]

[[package]]
name = "tm-store"
version = "0.1.0"
dependencies = [
 "gitdep",
 "rusqlite",
 "tempfile",
]
"#;

/// oakoss/tm's shape: `tm-cli` is the one versioned crate and ships
/// `tm-core`, which ships `tm-store`; both are `publish = false`. Writes
/// [`TM_ROOT_MANIFEST`] and [`TM_LOCK`] beside the members, uncommitted.
pub fn tm_workspace(root: &Path) {
    std::fs::write(root.join("Cargo.toml"), TM_ROOT_MANIFEST).expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), TM_LOCK).expect("Cargo.lock");
    for (name, manifest) in [
        (
            "tm-cli",
            "[package]\nname = \"tm-cli\"\nversion = \"0.1.0\"\nedition.workspace = true\n\n\
             [dependencies]\ntm-core = { path = \"../tm-core\" }\n",
        ),
        (
            "tm-core",
            "[package]\nname = \"tm-core\"\nversion = \"0.1.0\"\nedition.workspace = true\n\
             publish = false\n\n[dependencies]\ntm-store = { path = \"../tm-store\" }\n",
        ),
        (
            "tm-store",
            "[package]\nname = \"tm-store\"\nversion = \"0.1.0\"\nedition.workspace = true\n\
             publish = false\n\n[dependencies]\ngitdep.workspace = true\n\
             rusqlite.workspace = true\n\n[dev-dependencies]\ntempfile.workspace = true\n",
        ),
    ] {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).expect("member src");
        std::fs::write(dir.join("Cargo.toml"), manifest).expect("member Cargo.toml");
        std::fs::write(dir.join("src/lib.rs"), "").expect("member lib.rs");
    }
}

/// A root that is also a package: `app` ships the member `util` and the
/// registry crate `log`.
pub const ROOT_PACKAGE_MANIFEST: &str = r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
log = "0.4.20"
util = { path = "util", version = "0.1.0" }

[workspace]
members = ["util"]
"#;

pub const ROOT_PACKAGE_LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "log",
 "util",
]

[[package]]
name = "log"
version = "0.4.20"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"

[[package]]
name = "util"
version = "0.1.0"
"#;

/// Writes [`ROOT_PACKAGE_MANIFEST`], [`ROOT_PACKAGE_LOCK`] and `util`,
/// uncommitted.
pub fn root_package_workspace(root: &Path) {
    std::fs::write(root.join("Cargo.toml"), ROOT_PACKAGE_MANIFEST).expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), ROOT_PACKAGE_LOCK).expect("Cargo.lock");
    std::fs::create_dir_all(root.join("src")).expect("root src");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main.rs");
    std::fs::create_dir_all(root.join("util/src")).expect("util src");
    std::fs::write(
        root.join("util/Cargo.toml"),
        "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("util Cargo.toml");
    std::fs::write(root.join("util/src/lib.rs"), "").expect("util lib.rs");
}

/// What `cargo generate-lockfile --offline` wrote for [`patched_helper_workspace`]
/// in both layouts. `cargo tree --locked --offline` accepts it, and each of
/// [`PATCHED_HELPER_LOG_0_4_35`] and [`PATCHED_HELPER_MEMCHR_2_8_3`] applied.
pub const PATCHED_HELPER_LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "aho-corasick"
version = "1.1.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c982642fa9e8606056828ee9a8505737230110bb1099153c79efe865c59d12ba"
dependencies = [
 "memchr",
]

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "helper",
]

[[package]]
name = "helper"
version = "0.1.0"
dependencies = [
 "aho-corasick",
 "log",
]

[[package]]
name = "log"
version = "0.4.34"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f9f8bd3e56ce4dfc153cf470fffbfa98c7620958b312ca5c3a4b8d5181fd13c6"

[[package]]
name = "memchr"
version = "2.8.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f8ca58f447f06ed17d5fc4043ce1b10dd205e060fb3ce5b979b8ed8e59ff3f79"
"#;

/// `(from, to)` moving [`PATCHED_HELPER_LOCK`]'s `log`, which `helper` declares.
pub const PATCHED_HELPER_LOG_0_4_35: (&str, &str) = (
    "version = \"0.4.34\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
     checksum = \"f9f8bd3e56ce4dfc153cf470fffbfa98c7620958b312ca5c3a4b8d5181fd13c6\"",
    "version = \"0.4.35\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
     checksum = \"5d15f86d12da44cd8d4ec3c73984537eb7f4cadcb9776a575a0a8a05ca11005c\"",
);

/// `(from, to)` moving [`PATCHED_HELPER_LOCK`]'s `memchr`, which only
/// `aho-corasick` uses, so `helper`'s own row stays as it was.
pub const PATCHED_HELPER_MEMCHR_2_8_3: (&str, &str) = (
    "version = \"2.8.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
     checksum = \"f8ca58f447f06ed17d5fc4043ce1b10dd205e060fb3ce5b979b8ed8e59ff3f79\"",
    "version = \"2.8.3\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
     checksum = \"cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98\"",
);

/// `app` declares `helper = "0.1"` from the registry, and the root's
/// `[patch.crates-io]` points it at the `publish = false` member `helper`,
/// which uses `aho-corasick` and `log`. `app` is the root package when
/// `root_package`, else a member beside `helper`. Uncommitted.
pub fn patched_helper_workspace(root: &Path, root_package: bool) {
    let app = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
               [dependencies]\nhelper = \"0.1\"\n";
    let patch = "[patch.crates-io]\nhelper = { path = \"helper\" }\n";
    let (manifest, app_dir) = if root_package {
        (
            format!("{app}\n[workspace]\nmembers = [\"helper\"]\n\n{patch}"),
            None,
        )
    } else {
        (
            format!("[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"helper\"]\n\n{patch}"),
            Some(root.join("app")),
        )
    };
    std::fs::write(root.join("Cargo.toml"), manifest).expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), PATCHED_HELPER_LOCK).expect("Cargo.lock");
    if let Some(dir) = app_dir {
        std::fs::create_dir_all(dir.join("src")).expect("app src");
        std::fs::write(dir.join("Cargo.toml"), app).expect("app Cargo.toml");
        std::fs::write(dir.join("src/lib.rs"), "").expect("app lib.rs");
    } else {
        std::fs::create_dir_all(root.join("src")).expect("root src");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main.rs");
    }
    std::fs::create_dir_all(root.join("helper/src")).expect("helper src");
    std::fs::write(
        root.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         publish = false\n\n[dependencies]\naho-corasick = \"1\"\nlog = \"0.4\"\n",
    )
    .expect("helper Cargo.toml");
    std::fs::write(root.join("helper/src/lib.rs"), "").expect("helper lib.rs");
}

/// What `cargo generate-lockfile --offline` wrote for [`dev_alias_workspace`]
/// with the dev alias at `alias` (`0.13.0` or `0.14.0`), and a `shell` row
/// when it is the root package; `cargo tree --locked --offline --workspace`
/// accepts each.
pub fn dev_alias_lock(alias: &str, root_package: bool) -> String {
    let checksum = match alias {
        "0.13.0" => "413ee7dfc52ee1a4949ceeb7dbc8a33f2d6c088194d9f922fb8318faf1f01186",
        "0.14.0" => "2b192c782037fadd9cfa75548310488aabdbf3d2da73885b31bd0abd03351285",
        other => panic!("no lock recorded for itertools {other}"),
    };
    let shell = if root_package {
        "\n[[package]]\nname = \"shell\"\nversion = \"0.1.0\"\n"
    } else {
        ""
    };
    format!(
        r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "itertools 0.10.5",
 "itertools {alias}",
]

[[package]]
name = "either"
version = "1.19.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0e9c71c2167ca323c882b99918929403426e2373ea17242ff5653e0d5e1058be"

[[package]]
name = "itertools"
version = "0.10.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b0fd2260e829bddf4cb6ea802289de2f86d6a7a690192fbe91b3f46e0f2c8473"
dependencies = [
 "either",
]

[[package]]
name = "itertools"
version = "{alias}"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{checksum}"
dependencies = [
 "either",
]
{shell}"#
    )
}

/// The member `app` ships `itertools = "0.10"` and inherits a dev-only alias
/// of it at `alias_req`; the root is the package `shell` when `root_package`.
pub fn dev_alias_workspace(root: &Path, root_package: bool, alias_req: &str, alias_lock: &str) {
    let inherited = format!(
        "[workspace.dependencies]\nitertools13 = {{ package = \"itertools\", version = \"{alias_req}\" }}\n"
    );
    let manifest = if root_package {
        std::fs::create_dir_all(root.join("src")).expect("root src");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main.rs");
        format!(
            "[package]\nname = \"shell\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\nmembers = [\"app\"]\n\n{inherited}"
        )
    } else {
        format!("[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n\n{inherited}")
    };
    std::fs::write(root.join("Cargo.toml"), manifest).expect("root Cargo.toml");
    std::fs::write(
        root.join("Cargo.lock"),
        dev_alias_lock(alias_lock, root_package),
    )
    .expect("Cargo.lock");
    std::fs::create_dir_all(root.join("app/src")).expect("app src");
    std::fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nitertools = \"0.10\"\n\n\
         [dev-dependencies]\nitertools13.workspace = true\n",
    )
    .expect("app Cargo.toml");
    std::fs::write(root.join("app/src/lib.rs"), "").expect("app lib.rs");
}

/// What `cargo generate-lockfile --offline` wrote for a member `helper` and a
/// path crate outside the workspace, `vendor/helper`, each at its version
/// with its exact pick of `itertools`, used by `cli`. `cargo tree --locked
/// --offline` accepts it with `helper` at `0.1.0` picking `0.13.0` beside
/// `0.5.0` picking `0.14.0`, and at `0.2.0` picking `0.14.0` beside `0.6.0`
/// picking `0.13.0`.
pub fn helper_swap_lock(member: (&str, &str), outside: (&str, &str)) -> String {
    format!(
        r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "cli"
version = "0.1.0"
dependencies = [
 "helper {}",
]

[[package]]
name = "either"
version = "1.19.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0e9c71c2167ca323c882b99918929403426e2373ea17242ff5653e0d5e1058be"

[[package]]
name = "helper"
version = "{}"
dependencies = [
 "itertools {}",
]

[[package]]
name = "helper"
version = "{}"
dependencies = [
 "itertools {}",
]

[[package]]
name = "itertools"
version = "0.13.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "413ee7dfc52ee1a4949ceeb7dbc8a33f2d6c088194d9f922fb8318faf1f01186"
dependencies = [
 "either",
]

[[package]]
name = "itertools"
version = "0.14.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2b192c782037fadd9cfa75548310488aabdbf3d2da73885b31bd0abd03351285"
dependencies = [
 "either",
]
"#,
        outside.0, member.0, member.1, outside.0, outside.1
    )
}

/// The `helper` members and crates [`helper_swap_lock`] locks, written at those
/// versions and picks, uncommitted.
pub fn helper_swap_workspace(root: &Path, member: (&str, &str), outside: (&str, &str)) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"cli\", \"helper\"]\nexclude = [\"vendor/helper\"]\n",
    )
    .expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), helper_swap_lock(member, outside)).expect("Cargo.lock");
    for (dir, name, version, dependency) in [
        (
            "cli",
            "cli",
            "0.1.0",
            "helper = { path = \"../vendor/helper\" }".to_owned(),
        ),
        (
            "helper",
            "helper",
            member.0,
            format!("itertools = \"={}\"", member.1),
        ),
        (
            "vendor/helper",
            "helper",
            outside.0,
            format!("itertools = \"={}\"", outside.1),
        ),
    ] {
        std::fs::create_dir_all(root.join(dir).join("src")).expect("src");
        std::fs::write(
            root.join(dir).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n\
                 [dependencies]\n{dependency}\n"
            ),
        )
        .expect("manifest");
        std::fs::write(root.join(dir).join("src/lib.rs"), "").expect("lib.rs");
    }
}

/// What `cargo generate-lockfile --offline` wrote for [`tool_workspace`], with
/// the `xt` row once `tools/xt` is a member; `cargo tree --locked --offline
/// --workspace` accepts both.
pub fn tool_lock(joined: bool) -> String {
    let xt = if joined {
        "\n[[package]]\nname = \"xt\"\nversion = \"0.3.0\"\n"
    } else {
        ""
    };
    format!(
        r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "itertools",
]

[[package]]
name = "either"
version = "1.19.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0e9c71c2167ca323c882b99918929403426e2373ea17242ff5653e0d5e1058be"

[[package]]
name = "itertools"
version = "0.10.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b0fd2260e829bddf4cb6ea802289de2f86d6a7a690192fbe91b3f46e0f2c8473"
dependencies = [
 "either",
]
{xt}"#
    )
}

/// The member `app`, and `tools/xt`: excluded and its own workspace, which its
/// `version.workspace = true` reads, until `joined` makes it a member.
pub fn tool_workspace(root: &Path, joined: bool) {
    let (members, xt) = if joined {
        (
            "members = [\"app\", \"tools/xt\"]\n",
            "[package]\nname = \"xt\"\nversion = \"0.3.0\"\nedition = \"2021\"\n",
        )
    } else {
        (
            "members = [\"app\"]\nexclude = [\"tools/xt\"]\n",
            "[package]\nname = \"xt\"\nversion.workspace = true\nedition = \"2021\"\n\n\
             [workspace]\n\n[workspace.package]\nversion = \"0.3.0\"\n",
        )
    };
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\n{members}"),
    )
    .expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), tool_lock(joined)).expect("Cargo.lock");
    for (dir, manifest) in [
        (
            "app",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nitertools = \"0.10\"\n",
        ),
        ("tools/xt", xt),
    ] {
        std::fs::create_dir_all(root.join(dir).join("src")).expect("src");
        std::fs::write(root.join(dir).join("Cargo.toml"), manifest).expect("manifest");
        std::fs::write(root.join(dir).join("src/lib.rs"), "").expect("lib.rs");
    }
}

/// What `cargo generate-lockfile --offline` wrote for [`two_registries_workspace`]
/// with the dev registry's `shared` at `dev` (`1.1.0` or `1.2.0`), each
/// registry replaced by a vendored directory; `cargo tree --locked --offline -e
/// all` accepts both.
pub fn two_registries_lock(dev: &str) -> String {
    let checksum = match dev {
        "1.1.0" => "a33311b952cc17fb773acc98c70ab56fff47f71fdb33b9c967a57c2ad89d9b20",
        "1.2.0" => "05da7ac099bf8111f4089306eb8ca3b3bd30f25f8b43e6081e6fdb469e562272",
        other => panic!("no lock recorded for shared {other}"),
    };
    format!(
        r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "shared 1.0.0",
 "shared {dev}",
]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://corp.invalid/index"
checksum = "6e97dec1a852fb0fad6027630abda5bca9badd98287a61dbec5dc7bb8e166f6b"

[[package]]
name = "shared"
version = "{dev}"
source = "sparse+https://Other.invalid/index/"
checksum = "{checksum}"
"#
    )
}

/// The member `app` ships `shared` from the registry `corp` and tests with
/// `shared` from `other`, both at `*`; `.cargo/config.toml` names both indexes
/// in spellings cargo normalizes.
pub fn two_registries_workspace(root: &Path, dev: &str) {
    std::fs::create_dir_all(root.join(".cargo")).expect(".cargo");
    std::fs::write(
        root.join(".cargo/config.toml"),
        "[registries.corp]\nindex = \"https://Corp.invalid/a/../index\"\n\n\
         [registries.other]\nindex = \"sparse+https://Other.invalid/index/\"\n",
    )
    .expect("config.toml");
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n",
    )
    .expect("root Cargo.toml");
    std::fs::write(root.join("Cargo.lock"), two_registries_lock(dev)).expect("Cargo.lock");
    std::fs::create_dir_all(root.join("app/src")).expect("app src");
    std::fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nshared = { version = \"*\", registry = \"corp\" }\n\n\
         [dev-dependencies]\nshared-dev = { package = \"shared\", version = \"*\", registry = \"other\" }\n",
    )
    .expect("app Cargo.toml");
    std::fs::write(root.join("app/src/lib.rs"), "").expect("app lib.rs");
}

/// `--no-verify`: fixtures have no hooks.
pub fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "--no-verify", "-m", message]);
}

/// The exit code beside the streams. [`oakum_output`]'s bool collapses `1` and
/// `2`, which is the distinction `CliError::exit_code` exists to draw.
pub fn oakum_exit(root: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = oakum(root).args(args).output().expect("oakum");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

pub fn oakum_output(root: &Path, args: &[&str]) -> (bool, String, String) {
    let out = oakum(root).args(args).output().expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `command`'s output, or `None` once the child itself outlives `limit`, when it
/// is killed: a test that a look does not block must not hang the suite when it
/// does. The pipes drain while it runs, so a full pipe is not mistaken for a hang.
#[cfg(unix)]
pub fn output_within(command: &mut Command, limit: std::time::Duration) -> Option<Output> {
    fn drain(mut pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).expect("read pipe");
            bytes
        })
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let stdout = drain(child.stdout.take().expect("stdout was piped"));
    let stderr = drain(child.stderr.take().expect("stderr was piped"));
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    Some(Output {
        status,
        stdout: stdout.join().expect("stdout drained"),
        stderr: stderr.join().expect("stderr drained"),
    })
}

/// The pins [`mock_action_pins`] answers with, as the printed workflow carries
/// them: `<commit> # <tag>`.
pub const CHECKOUT_PIN: &str = "9999999999999999999999999999999999999999 # v9.9.9";
pub const PNPM_SETUP_PIN: &str = "8888888888888888888888888888888888888888 # v8.8.8";
pub const UPLOAD_PIN: &str = "7777777777777777777777777777777777777777 # v7.7.7";
pub const DOWNLOAD_PIN: &str = "6666666666666666666666666666666666666666 # v6.6.6";

/// Answers GitHub's latest-release and tag-commit reads for `repo` with `pin`.
pub fn mock_release_pin(server: &httpmock::MockServer, repo: &str, pin: &str) {
    let (commit, tag) = pin.split_once(" # ").expect("`<commit> # <tag>`");
    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/repos/{repo}/releases/latest"));
        then.status(200)
            .json_body(serde_json::json!({ "tag_name": tag }));
    });
    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/repos/{repo}/commits/refs/tags/{tag}"));
        then.status(200)
            .json_body(serde_json::json!({ "sha": commit }));
    });
}

pub fn mock_action_pins() -> httpmock::MockServer {
    let server = httpmock::MockServer::start();
    mock_release_pin(&server, "actions/checkout", CHECKOUT_PIN);
    mock_release_pin(&server, "pnpm/action-setup", PNPM_SETUP_PIN);
    mock_release_pin(&server, "actions/upload-artifact", UPLOAD_PIN);
    mock_release_pin(&server, "actions/download-artifact", DOWNLOAD_PIN);
    server
}

/// Deadline tests run one at a time: contention among them is what their
/// two-second budget cannot absorb. A poisoned lock is recovered, since the
/// panic that poisoned it already failed its own test.
#[cfg(unix)]
pub fn deadline_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
