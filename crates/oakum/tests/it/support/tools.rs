//! The programs a child under test may find, and the shims a test puts ahead
//! of them.

#[cfg(unix)]
use std::ffi::OsStr;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
use super::fixture::sibling;
use super::fixture::Fixture;

/// Writes `content` beside `path`, then installs it exec-bit-set from a
/// subprocess. `fs::write` here would hold a write fd in this test process;
/// every concurrent test's fork inherits it, and a child exec'ing the file
/// inside that window dies with ETXTBSY.
#[cfg(unix)]
pub fn install_executable(path: &Path, content: impl AsRef<str>) {
    let source = path.with_file_name("installed.source");
    std::fs::write(&source, content.as_ref()).expect("executable source");
    let installed = Command::new("sh")
        .args(["-c", r#"cat "$1" > "$2" && chmod 755 "$2""#, "sh"])
        .arg(&source)
        .arg(path)
        .status()
        .expect("install executable")
        .success();
    assert!(installed, "installing {} failed", path.display());
}

/// Fake ssh that logs argv to `log` and exits 255. `GIT_TERMINAL_PROMPT=0` does
/// not reach ssh (it reads `/dev/tty` for host keys), so this proves `BatchMode`
/// without a network.
#[cfg(unix)]
pub fn recording_fake_ssh(root: &Path, log: &Path) -> PathBuf {
    let script = root.join("fake-ssh");
    install_executable(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexit 255\n",
            log.display()
        ),
    );
    script
}

/// Writes `name` as an executable in the fixture's `shim` sibling and returns
/// the directory; a second call adds another shim beside the first.
#[cfg(unix)]
pub fn path_shim(root: &Fixture, name: &str, script: impl AsRef<str>) -> PathBuf {
    let dir = sibling(root, "shim");
    std::fs::create_dir_all(&dir).expect("shim dir");
    install_executable(&dir.join(name), script);
    dir
}

pub fn path_prefixed_by(first: &Path) -> std::ffi::OsString {
    let rest = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(first.to_path_buf()).chain(std::env::split_paths(&rest)))
        .expect("PATH entries")
}

/// What a child under test may find on a PATH that carries nothing else:
/// the tools discovery spawns, the shell the shims run under, and the
/// interpreter the pseudo-TTY harness runs `oakum` from.
#[cfg(unix)]
pub const HERMETIC_TOOLS: [&str; 6] = ["git", "cargo", "pnpm", "node", "sh", "python3"];

/// Each hermetic tool as the ambient PATH resolves it, found once per test
/// binary. A tool the machine lacks is left out; a child that needs it fails
/// as it would on the ambient PATH.
#[cfg(unix)]
fn ambient_tools() -> &'static [(&'static str, PathBuf)] {
    static RESOLVED: std::sync::OnceLock<Vec<(&'static str, PathBuf)>> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(|| {
        HERMETIC_TOOLS
            .iter()
            .filter_map(|name| find_on_ambient_path(name).map(|found| (*name, found)))
            .collect()
    })
}

#[cfg(unix)]
fn find_on_ambient_path(name: &str) -> Option<PathBuf> {
    find_on_path(&std::env::var_os("PATH").unwrap_or_default(), name)
}

/// What `command -v` answers on `path`: the first entry this process may
/// execute, so a file earlier on it that lacks the process's execute bit does
/// not shadow the real tool. Asked of `sh` itself rather than re-derived from
/// mode bits, which would need the process's identity to read correctly.
/// Deliberately stricter than `discover::child::find_program`, which takes
/// the first file by name: this resolves what a shell would run.
#[cfg(unix)]
pub fn find_on_path(path: &OsStr, name: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    // `LC_ALL=C`: GNU bash warns on stderr for a locale it cannot apply
    // (inferred; this host's `/bin/sh` is quiet), and the check below would
    // read that as the probe failing.
    let out = Command::new("/bin/sh")
        .args(["-c", r#"command -v -- "$1""#, "sh", name])
        .env("PATH", path)
        .env("LC_ALL", "C")
        .output()
        .unwrap_or_else(|err| panic!("spawn /bin/sh to resolve `{name}`: {err}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Not found is a normal, quiet exit; anything else is the probe failing,
    // which must not read as an answer.
    assert!(
        out.status.code().is_some() && stderr.is_empty(),
        "`command -v {name}` under /bin/sh: {}, stderr {stderr:?}",
        out.status
    );
    if !out.status.success() {
        return None;
    }
    let mut answer = out.stdout;
    if answer.last() == Some(&b'\n') {
        answer.pop();
    }
    let found = PathBuf::from(std::ffi::OsString::from_vec(answer));
    let answer = found.display().to_string();
    assert!(
        found.is_absolute(),
        "`command -v {name}` answered {answer:?}, not a path on PATH"
    );
    Some(found)
}

#[cfg(unix)]
pub fn ambient_tool(name: &str) -> PathBuf {
    find_on_ambient_path(name).unwrap_or_else(|| panic!("`{name}` is not on PATH"))
}

/// A PATH for a child of `root` that sees only [`HERMETIC_TOOLS`]: a `bin`
/// sibling of symlinks to where the ambient PATH resolves each, with `shim`
/// ahead of it when given. Set as the whole PATH, not a prefix, so a release
/// tool installed on the machine — a global `@changesets/cli`, a `knope` beside
/// `cargo` — cannot reach the child. Filtering the ambient PATH instead would
/// drop `cargo` with `knope`, since `cargo install` writes both to one
/// directory.
#[cfg(unix)]
pub fn hermetic_path(root: &Fixture, shim: Option<&Path>) -> std::ffi::OsString {
    let bin = root.hermetic_bin.get_or_init(|| {
        let bin = root.container().join("bin");
        std::fs::create_dir(&bin).expect("hermetic bin");
        for name in HERMETIC_TOOLS {
            if ambient_tools().iter().any(|(found, _)| *found == name) {
                continue;
            }
            // Without these three no fixture can run at all, so say so here
            // rather than from inside the child; the rest fail naming themselves.
            assert!(
                !["git", "cargo", "sh"].contains(&name),
                "hermetic PATH for {} needs `{name}`, which is not on the ambient PATH",
                root.display()
            );
            eprintln!(
                "hermetic PATH for {} lacks `{name}`: not on the ambient PATH",
                root.display()
            );
        }
        for (name, target) in ambient_tools() {
            std::os::unix::fs::symlink(target, bin.join(name))
                .unwrap_or_else(|err| panic!("link {name} into {}: {err}", bin.display()));
        }
        bin
    });
    std::env::join_paths(
        shim.map(Path::to_path_buf)
            .into_iter()
            .chain(std::iter::once(bin.clone())),
    )
    .expect("PATH entries")
}

/// Windows has no symlink a test can create without privilege, so the child
/// keeps the ambient PATH there. Inferred, not measured: this host is not
/// Windows.
#[cfg(not(unix))]
pub fn hermetic_path(_root: &Fixture, shim: Option<&Path>) -> std::ffi::OsString {
    match shim {
        Some(dir) => path_prefixed_by(dir),
        None => std::env::var_os("PATH").unwrap_or_default(),
    }
}
