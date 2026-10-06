//! Child processes oakum waits on under a wall-clock deadline.
//!
//! A child that blocks — a credential helper, a signing program reading
//! `/dev/tty`, a tool opening a FIFO — would otherwise hang oakum with no
//! output, so every child runs through [`output_within`] and the caller words
//! its own [`ChildFailure`]. It lives under `discover`, the library's one I/O
//! module (ADR-0002); the CLI's git children use it too.

use std::io::{self, Read};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

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

/// Why a child produced no [`Output`].
#[derive(Debug)]
pub enum ChildFailure {
    Spawn(io::Error),
    /// The deadline expired and oakum killed the child.
    Deadline {
        limit: Duration,
    },
    /// The child exited, but something it spawned still held its pipes open
    /// when the deadline ran out, so the output could not be collected.
    /// Nothing was killed and the child's status is in hand.
    DrainStalled {
        limit: Duration,
        status: ExitStatus,
    },
    /// Waiting on a spawned child failed; oakum killed it on the way out.
    Wait(io::Error),
    /// Reading a pipe failed partway, so no truncated reply is passed on.
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
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(ChildFailure::Spawn)?;
    let stdout = drain(child.stdout.take().expect("stdout was piped"));
    let stderr = drain(child.stderr.take().expect("stderr was piped"));
    let started = Instant::now();
    let status = loop {
        let waited = match child.try_wait() {
            Ok(waited) => waited,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ChildFailure::Wait(err));
            }
        };
        match waited {
            Some(status) => break status,
            None if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ChildFailure::Deadline { limit });
            }
            // Each child waits out part of a poll after exiting; a backoff
            // from 100 µs measured slower than this fixed 1 ms.
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    };
    collect_drains(status, stdout, stderr, limit, started)
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

    use super::{collect_drains, ChildFailure};

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
