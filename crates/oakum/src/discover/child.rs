//! Child processes oakum waits on under a wall-clock deadline.
//!
//! A child that blocks — a credential helper, a signing program reading
//! `/dev/tty`, a tool opening a FIFO — would otherwise hang oakum with no
//! output, so every child runs through [`output_within`] and the caller words
//! its own [`ChildFailure`]. It lives under `discover`, the library's one I/O
//! module (ADR-0002); the CLI's git children use it too.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::descendants::kill_tree;
pub use super::descendants::Reach;

/// Generous, because a tag push of a large repository or a first `cargo` run
/// that installs a pinned toolchain is legitimately slow; the point is bounding
/// the unbounded, not being tight.
pub const DEFAULT_DEADLINE: Duration = Duration::from_mins(5);

/// The variable that overrides [`DEFAULT_DEADLINE`] for every child oakum
/// waits on. Named for the remote operations it first bounded; an env var
/// rather than a config key because it belongs to the machine and the moment
/// (CI timeout budgets, one slow push), not to the repository.
pub const DEADLINE_VAR: &str = "OAKUM_REMOTE_DEADLINE";

/// The deadline in force: [`DEADLINE_VAR`] when set, else [`DEFAULT_DEADLINE`].
///
/// # Errors
///
/// The variable is set to something other than a positive whole number of
/// seconds; the message names it and the value.
pub fn deadline() -> Result<Duration, String> {
    match std::env::var_os(DEADLINE_VAR) {
        None => Ok(DEFAULT_DEADLINE),
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map(Duration::from_secs)
            .ok_or_else(|| {
                format!(
                    "{DEADLINE_VAR} must be a positive whole number of seconds, got `{}`",
                    value.to_string_lossy()
                )
            }),
    }
}

/// What a child wrote, as text for a message: colour codes dropped and CRLF
/// made LF. pnpm colours its errors even into a pipe, and oakum escapes what
/// reaches the terminal, so left in they print as `\u{1b}[31m` noise. Dropping
/// a colour code can only reveal text; any other control byte stays for the
/// printer to escape.
#[must_use]
pub fn said(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("\u{1b}[") {
        out.push_str(&rest[..at]);
        let params = &rest[at + 2..];
        let len = params
            .find(|ch: char| !(ch.is_ascii_digit() || ch == ';' || ch == ':'))
            .unwrap_or(params.len());
        if params[len..].starts_with('m') {
            rest = &params[len + 1..];
        } else {
            out.push('\u{1b}');
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// Why a child that failed says it failed: its stderr, else its stdout, else
/// a note that it said nothing.
pub(crate) fn complaint(output: &Output) -> String {
    [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| said(bytes))
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| String::from("no stderr or stdout"))
}

/// Where `name` is on `PATH`. Spawn the result by full path: `CreateProcess`
/// consults no PATHEXT, so `Command::new("pnpm")` finds `pnpm.exe` only and
/// misses a `.cmd` shim (measured on GHA `windows-latest`).
#[must_use]
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| String::from(".COM;.EXE;.BAT;.CMD"));
    resolve_on_path(name, &path, &pathext, cfg!(windows))
}

/// On Windows, the first `name` plus a `pathext` extension in each directory,
/// never the bare name: `CreateProcess` cannot run an extensionless file, and
/// npm writes one beside each `.cmd` shim. Elsewhere, the bare name.
fn resolve_on_path(name: &str, path: &OsStr, pathext: &str, windows: bool) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let names: Vec<String> = if windows {
        pathext
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| format!("{name}{ext}"))
            .collect()
    } else {
        Vec::from([name.to_owned()])
    };
    path_dirs(path, windows)
        .into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Git Bash's shapes are read only on Windows: elsewhere `/a/bin` is a
/// directory, not drive `A:`. An empty entry is skipped rather than read as
/// the working directory, which is the repository being inspected.
fn path_dirs(path: &OsStr, windows: bool) -> Vec<PathBuf> {
    if windows {
        path_entries(path)
    } else {
        std::env::split_paths(path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .collect()
    }
}

/// Git Bash `PATH` can be `;`-separated, `:`-separated `/c/Users/...`, or mixed
/// `C:\cargo\bin:/c/Users/...`. Native `split_paths` on Windows only splits on `;`.
/// Quotes are stripped and a `;` inside them is part of the name, as Windows
/// reads `"C:\Program Files\x"`.
fn path_entries(path: &OsStr) -> Vec<PathBuf> {
    let raw = path.to_string_lossy();
    let mut entries = vec![String::new()];
    let mut in_quotes = false;
    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => entries.push(String::new()),
            _ => entries.last_mut().expect("starts with one").push(ch),
        }
    }
    entries
        .iter()
        .flat_map(|entry| split_path_chunk(entry))
        .filter(|chunk| !chunk.is_empty())
        .map(msys_dir)
        .collect()
}

/// Split on `:` that is a PATH separator, not a Windows drive letter (`C:`,
/// or `\\?\C:` and `\\.\C:` in a verbatim path).
fn split_path_chunk(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let entry = &s[start..i];
            let letter = entry
                .strip_prefix(r"\\?\")
                .or_else(|| entry.strip_prefix(r"\\.\"))
                .unwrap_or(entry);
            let is_drive = letter.len() == 1 && letter.as_bytes()[0].is_ascii_alphabetic();
            if !is_drive {
                parts.push(&s[start..i]);
                start = i + 1;
            }
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

fn msys_dir(entry: &str) -> PathBuf {
    let bytes = entry.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b'/' {
        let drive = (bytes[1] as char).to_ascii_uppercase();
        PathBuf::from(format!("{}:\\{}", drive, entry[3..].replace('/', "\\")))
    } else {
        PathBuf::from(entry)
    }
}

/// Why a child produced no [`Output`].
#[derive(Debug)]
pub enum ChildFailure {
    Spawn(io::Error),
    /// The deadline expired and oakum killed the child and what it started.
    Deadline {
        limit: Duration,
        reach: Reach,
    },
    /// The child exited, but something it spawned still held its pipes open
    /// when the deadline ran out, so the output could not be collected.
    /// Nothing was killed and the child's status is in hand.
    DrainStalled {
        limit: Duration,
        status: ExitStatus,
    },
    /// Waiting on a spawned child failed; oakum killed it and what it started
    /// on the way out.
    Wait(io::Error, Reach),
    /// A pipe failed partway: reading the reply, or writing the input to a
    /// child that still exited 0. No truncated exchange is passed on.
    Read(io::Error),
}

/// Runs `command` with stdin closed and stdout/stderr captured, killing it
/// once `limit` passes.
///
/// The pipes are drained on their own threads so a child writing more than a
/// pipe buffer cannot deadlock against the timed wait, and the drained bytes
/// arrive through channels so collecting them is bounded by the same deadline:
/// a grandchild can inherit the pipes and hold them open past the child's own
/// exit. On expiry the drain threads are abandoned.
///
/// # Errors
///
/// See [`ChildFailure`].
///
/// # Panics
///
/// Never: stdout and stderr are set to piped just before the spawn.
pub fn output_within(command: &mut Command, limit: Duration) -> Result<Output, ChildFailure> {
    output_within_reading(command, limit, None)
}

/// [`output_within`], writing `input` to stdin on its own thread so a child
/// that answers before reading it all cannot deadlock against the write.
///
/// # Errors
///
/// See [`ChildFailure`].
///
/// # Panics
///
/// Never: every stream it takes is set to piped just before the spawn.
pub fn output_within_reading(
    command: &mut Command,
    limit: Duration,
    input: Option<&[u8]>,
) -> Result<Output, ChildFailure> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(ChildFailure::Spawn)?;
    let written = input.map(|input| {
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let input = input.to_vec();
        let (sent, written) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(io::Write::write_all(&mut stdin, &input));
        });
        written
    });
    let stdout = drain(child.stdout.take().expect("stdout was piped"));
    let stderr = drain(child.stderr.take().expect("stderr was piped"));
    let started = Instant::now();
    let status = loop {
        let waited = match child.try_wait() {
            Ok(waited) => waited,
            Err(err) => {
                let reach = kill_tree(&mut child);
                return Err(ChildFailure::Wait(err, reach));
            }
        };
        match waited {
            Some(status) => break status,
            None if started.elapsed() >= limit => {
                let reach = kill_tree(&mut child);
                return Err(ChildFailure::Deadline { limit, reach });
            }
            // Each child waits out part of a poll after exiting; a backoff
            // from 100 µs measured slower than this fixed 1 ms.
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    };
    let output = collect_drains(status, stdout, stderr, limit, started)?;
    // A child that exited 0 while its input could not be written answered a
    // question it was never fully asked; a failed one says why itself. The
    // wait shares the deadline, since a descendant can hold stdin open.
    let unwritten = written.and_then(|written| {
        let remaining = limit
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(50));
        match written.recv_timeout(remaining) {
            Ok(Ok(())) => None,
            Ok(Err(err)) => Some(ChildFailure::Read(err)),
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(ChildFailure::Read(
                io::Error::other("the stdin writer stopped without finishing"),
            )),
            Err(mpsc::RecvTimeoutError::Timeout) => Some(ChildFailure::DrainStalled {
                limit,
                status: output.status,
            }),
        }
    });
    match unwritten {
        Some(failure) if output.status.success() => Err(failure),
        _ => Ok(output),
    }
}

/// Split from [`output_within`] so `DrainStalled` and `Read` are exercisable
/// with fake receivers in milliseconds.
fn collect_drains(
    status: ExitStatus,
    stdout: Receiver<io::Result<Vec<u8>>>,
    stderr: Receiver<io::Result<Vec<u8>>>,
    limit: Duration,
    started: Instant,
) -> Result<Output, ChildFailure> {
    let mut streams = Vec::new();
    for drained in [stdout, stderr] {
        // A small floor so a child that exits on the buzzer is not misreported
        // as a stalled drain: its bytes are already queued, and the grace only
        // covers collecting them.
        let remaining = limit
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(50));
        match drained.recv_timeout(remaining) {
            Ok(Ok(bytes)) => streams.push(bytes),
            Ok(Err(err)) => return Err(ChildFailure::Read(err)),
            Err(_) => return Err(ChildFailure::DrainStalled { limit, status }),
        }
    }
    let stderr = streams.pop().expect("two streams were pushed");
    let stdout = streams.pop().expect("two streams were pushed");
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn drain(mut stream: impl Read + Send + 'static) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let _ = sender.send(stream.read_to_end(&mut collected).map(|_| collected));
    });
    receiver
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::process::Command;

    #[cfg(unix)]
    use super::output_within_reading;
    use super::{
        collect_drains, complaint, path_dirs, path_entries, resolve_on_path, said, ChildFailure,
    };
    use crate::test_fixture::Fixture;

    fn scratch(label: &str) -> Fixture {
        Fixture::new("child", label)
    }

    #[test]
    fn resolve_on_path_prefers_pathext_order() {
        let dir = scratch("pathext-order");
        fs::write(dir.join("pnpm.CMD"), []).expect("cmd");
        fs::write(dir.join("pnpm.EXE"), []).expect("exe");
        let path = std::env::join_paths([&*dir]).expect("PATH");
        let found = resolve_on_path("pnpm", &path, ".COM;.EXE;.BAT;.CMD", true).expect("hit");
        assert_eq!(found.file_name(), Some(OsStr::new("pnpm.EXE")));
    }

    #[test]
    fn resolve_on_path_takes_the_bare_name_only_off_windows() {
        let dir = scratch("pathext-bare");
        fs::write(dir.join("pnpm"), []).expect("bare");
        fs::write(dir.join("pnpm.EXE"), []).expect("exe");
        let path = std::env::join_paths([&*dir]).expect("PATH");
        let found = resolve_on_path("pnpm", &path, ".COM;.EXE;.BAT;.CMD", false).expect("hit");
        assert_eq!(found.file_name(), Some(OsStr::new("pnpm")));
        fs::remove_file(dir.join("pnpm.EXE")).expect("rm exe");
        assert_eq!(
            resolve_on_path("pnpm", &path, ".COM;.EXE;.BAT;.CMD", true),
            None
        );
    }

    #[test]
    fn resolve_on_path_misses_when_nothing_matches() {
        let dir = scratch("pathext-miss");
        let path = std::env::join_paths([&*dir]).expect("PATH");
        assert_eq!(
            resolve_on_path("pnpm", &path, ".COM;.EXE;.BAT;.CMD", true),
            None
        );
    }

    #[test]
    fn resolve_on_path_walks_mixed_win32_and_unix_colon() {
        let dir = scratch("pathext-mixed");
        fs::write(dir.join("pnpm.EXE"), []).expect("exe");
        let mixed = format!(r"C:\does-not-exist:{}", dir.display());
        let found =
            resolve_on_path("pnpm", OsStr::new(&mixed), ".COM;.EXE;.BAT;.CMD", true).expect("hit");
        assert_eq!(found, dir.join("pnpm.EXE"));
    }

    /// Unix `split_paths` keeps a `;` inside one entry, so only the Windows
    /// reader finds this on every host.
    #[test]
    fn resolve_on_path_reads_a_semicolon_path_on_windows() {
        let dir = scratch("pathext-semicolon");
        fs::write(dir.join("pnpm.EXE"), []).expect("exe");
        let semi = format!(r"C:\does-not-exist;{}", dir.display());
        let found =
            resolve_on_path("pnpm", OsStr::new(&semi), ".COM;.EXE;.BAT;.CMD", true).expect("hit");
        assert_eq!(found, dir.join("pnpm.EXE"));
    }

    #[test]
    fn a_quoted_entry_is_one_native_directory() {
        let dirs = path_entries(OsStr::new(r#""C:\Program Files\knope";"D:\a;b";/c/tools"#));
        assert_eq!(
            dirs,
            [
                PathBuf::from(r"C:\Program Files\knope"),
                PathBuf::from(r"D:\a;b"),
                PathBuf::from(r"C:\tools"),
            ]
        );
        assert_eq!(
            path_entries(OsStr::new(r#""C:\open"#)),
            [PathBuf::from(r"C:\open")]
        );
        assert_eq!(
            path_entries(OsStr::new(r"\\?\C:\tools;\\.\D:\bin:/c/x")),
            [
                PathBuf::from(r"\\?\C:\tools"),
                PathBuf::from(r"\\.\D:\bin"),
                PathBuf::from(r"C:\x"),
            ]
        );
    }

    /// An empty entry would otherwise be the working directory: the repository
    /// being inspected, whose own `pnpm` must not run.
    #[test]
    fn an_empty_entry_is_no_directory() {
        let native = std::env::join_paths(["", "/usr/bin", ""]).expect("PATH");
        assert_eq!(path_dirs(&native, false), [PathBuf::from("/usr/bin")]);
        assert_eq!(
            path_entries(OsStr::new(";;C:\\bin;")),
            [PathBuf::from(r"C:\bin")]
        );
    }

    #[test]
    fn a_one_letter_directory_is_a_drive_only_on_windows() {
        let path = OsStr::new("/a/bin");
        assert_eq!(path_dirs(path, false), [PathBuf::from("/a/bin")]);
        assert_eq!(path_dirs(path, true), [PathBuf::from(r"A:\bin")]);
    }

    #[test]
    fn path_entries_converts_msys_drive_prefix() {
        let dirs = path_entries(OsStr::new("/c/Users/foo/mise/shims"));
        assert_eq!(dirs, [PathBuf::from(r"C:\Users\foo\mise\shims")]);
    }

    #[test]
    fn path_entries_splits_semicolon_mixed_with_msys() {
        let dirs = path_entries(OsStr::new(r"C:\cargo\bin;/c/Users/foo/shims"));
        assert_eq!(
            dirs,
            [
                PathBuf::from(r"C:\cargo\bin"),
                PathBuf::from(r"C:\Users\foo\shims"),
            ]
        );
    }

    #[test]
    fn path_entries_splits_colon_msys_path() {
        let dirs = path_entries(OsStr::new("/c/Users/foo:/usr/bin"));
        assert_eq!(
            dirs,
            [PathBuf::from(r"C:\Users\foo"), PathBuf::from("/usr/bin")]
        );
    }

    #[test]
    fn path_entries_splits_mixed_win32_and_msys_colon() {
        let dirs = path_entries(OsStr::new(
            r"C:\Users\foo\.cargo\bin:/c/Users/foo/mise/installs/pnpm/11.25.0",
        ));
        assert_eq!(
            dirs,
            [
                PathBuf::from(r"C:\Users\foo\.cargo\bin"),
                PathBuf::from(r"C:\Users\foo\mise\installs\pnpm\11.25.0"),
            ]
        );
    }

    #[test]
    fn a_complaint_is_stderr_else_stdout_else_a_note() {
        let output = |stderr: &[u8], stdout: &[u8]| std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        };
        assert_eq!(complaint(&output(b"err\n", b"out")), "err");
        assert_eq!(complaint(&output(b" \x1b[0m\n", b"\x1b[31mout")), "out");
        assert_eq!(complaint(&output(b"", b"")), "no stderr or stdout");
    }

    /// Colour as pnpm 12 writes it into a pipe goes; a cursor move stays for
    /// the printer to escape.
    #[test]
    fn said_drops_colour_and_crlf_and_keeps_other_escapes() {
        assert_eq!(
            said(b"Error: \x1b[31mERR_PNPM_X\x1b[0m\r\n  \x1b[1;31m\xc3\x97\x1b[39;49m done\r\n"),
            "Error: ERR_PNPM_X\n  \u{d7} done"
        );
        assert_eq!(said(b"a\x1b[2Jb\x1b[c\x1b"), "a\u{1b}[2Jb\u{1b}[c\u{1b}");
        assert_eq!(said(b"a\rb"), "a\rb");
    }

    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code.cast_unsigned())
        }
    }

    /// A disconnected receiver fails immediately — no wall-clock grace — and
    /// that path is `DrainStalled`, the same as a timed-out drain.
    #[test]
    fn collect_drains_reports_a_stalled_pipe() {
        let (drop_out, stdout) = std::sync::mpsc::channel::<std::io::Result<Vec<u8>>>();
        let (_keep_err, stderr) = std::sync::mpsc::channel();
        drop(drop_out);
        let err = collect_drains(
            exit_status(0),
            stdout,
            stderr,
            Duration::from_secs(5),
            Instant::now(),
        )
        .expect_err("a dropped drain is stalled");
        assert!(
            matches!(err, ChildFailure::DrainStalled { limit, .. } if limit == Duration::from_secs(5)),
            "{err:?}"
        );
    }

    /// `true` exits 0 without reading stdin, so a megabyte of input breaks
    /// the pipe while the child reports success.
    #[cfg(unix)]
    #[test]
    fn input_a_successful_child_never_took_is_a_failed_exchange() {
        let input = vec![b'x'; 1 << 20];
        let err = output_within_reading(
            &mut Command::new("true"),
            Duration::from_secs(5),
            Some(&input),
        )
        .expect_err("an unread input is not an answer");
        assert!(matches!(err, ChildFailure::Read(_)), "{err:?}");
    }

    /// A child that fails before taking its input says why itself; the
    /// broken pipe must not replace that.
    #[cfg(unix)]
    #[test]
    fn a_failed_child_keeps_its_own_diagnostic_over_unwritten_input() {
        let input = vec![b'x'; 1 << 20];
        let output = output_within_reading(
            Command::new("sh").args(["-c", "echo why >&2; exit 3"]),
            Duration::from_secs(5),
            Some(&input),
        )
        .expect("the child's own answer");
        assert!(!output.status.success());
        assert_eq!(output.stderr, b"why\n");
    }

    /// A descendant holding stdin unread cannot hold oakum past the
    /// deadline. dash gives a background job `/dev/null` before applying
    /// `<&0`, so the pipe goes through fd 3.
    #[cfg(unix)]
    #[test]
    fn a_descendant_holding_stdin_is_bounded_by_the_deadline() {
        let input = vec![b'x'; 1 << 20];
        let started = Instant::now();
        let err = output_within_reading(
            Command::new("sh").args(["-c", "exec 3<&0; sleep 5 <&3 >/dev/null 2>&1 & exit 0"]),
            Duration::from_millis(300),
            Some(&input),
        )
        .expect_err("stalled");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert!(matches!(err, ChildFailure::DrainStalled { .. }), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn input_the_child_takes_is_answered() {
        let output = output_within_reading(
            &mut Command::new("cat"),
            Duration::from_secs(5),
            Some(b"hi"),
        )
        .expect("cat answers");
        assert_eq!(output.stdout, b"hi");
    }

    #[test]
    fn collect_drains_reports_a_read_failure() {
        let (out_tx, stdout) = std::sync::mpsc::channel();
        let (err_tx, stderr) = std::sync::mpsc::channel();
        out_tx.send(Ok(b"ok".to_vec())).expect("stdout queued");
        err_tx
            .send(Err(std::io::Error::other("pipe broke")))
            .expect("stderr queued");
        let err = collect_drains(
            exit_status(0),
            stdout,
            stderr,
            Duration::from_secs(5),
            Instant::now(),
        )
        .expect_err("a failed read is Read");
        assert!(
            matches!(err, ChildFailure::Read(ref inner) if inner.to_string().contains("pipe broke")),
            "{err:?}"
        );
    }
}
