//! Git children built so they do not stop at a prompt.
//!
//! Oakum spawns git with piped standard handles and waits for it, but the child
//! still inherits the controlling terminal. A prompt on `/dev/tty` is an
//! indefinite hang rather than a failure — measured at over 20s against both an
//! https remote without cached credentials and an ssh remote whose host key is
//! unknown. Without a controlling terminal, git already fails on its own.
//!
//! Five sources can prompt. Three are answered here:
//!
//! - Git's terminal prompt: `GIT_TERMINAL_PROMPT=0`.
//! - Git's askpass chain (`GIT_ASKPASS`, `core.askPass`, `SSH_ASKPASS`):
//!   `GIT_ASKPASS=""`. The terminal-prompt variable does not reach it — with
//!   prompts disabled, git still invoked an askpass helper twice for an https
//!   credential.
//! - Ssh, which reads `/dev/tty` directly: `BatchMode=yes`.
//!
//! The fourth is a `credential.helper`, which runs with this environment applied
//! and can still block — measured at 8.7s against a helper that sleeps, and
//! unbounded for one that reads `/dev/tty`. Suppressing helpers is not an option
//! because they are what makes stored credentials authenticate.
//!
//! The fifth is signing. `git tag` with `tag.gpgSign` runs `gpg.program`, and
//! `gpg.format = ssh` runs `ssh-keygen -Y sign`. Every variable above reaches
//! the signing child — measured, it logged `GIT_TERMINAL_PROMPT=[0]
//! GIT_ASKPASS=[] GIT_SSH_COMMAND=[ssh -o BatchMode=yes]` — and none stops it:
//! a `gpg.program` that opens `/dev/tty` was invoked and never returned. It is
//! not a remote operation either, so the ssh transport handling never applies.
//!
//! Only a deadline on the child covers these, along with an interactive
//! `ProxyCommand`, which `BatchMode=yes` does not stop either.
//! [`DeadlinedGit::output`] carries that deadline for every git child oakum
//! spawns — signing included, since a signing child is local-classed and git
//! dials sockets from local-classed children too.

use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Output};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::Reply;

/// A git child carrying the no-prompt environment and the wall-clock
/// deadline. Opaque so neither can be stripped back off between construction
/// and the run. Every child rides it: git opens sockets — and spawns signing
/// programs — on its own schedule, so bounding only the remote-classed
/// children leaves a partial clone's lazy fetch, or a `gpg` that reads
/// `/dev/tty`, unbounded.
pub(super) struct DeadlinedGit(Command);

/// Why a git child produced no `Output`.
pub(super) enum RemoteFailure {
    Spawn(io::Error),
    /// `OAKUM_REMOTE_DEADLINE` was set to something that is not a positive
    /// whole number of seconds. Its own variant so a config mistake is never
    /// reported as "could not run git": git was never run.
    BadDeadline(String),
    /// The wall-clock deadline expired and oakum killed the child. The prompt
    /// sources no environment variable suppresses — a credential helper, an
    /// interactive `ProxyCommand`, a signing program that reads `/dev/tty` —
    /// block exactly here, and the deadline is the only mechanism that covers
    /// them and any future one.
    Deadline {
        limit: Duration,
    },
    /// The child exited, but something it spawned still held its pipes open
    /// when the deadline ran out, so the output could not be collected. Its
    /// own variant because nothing was killed and the child's status is in
    /// hand — reporting this as a kill would misdescribe both.
    DrainStalled {
        limit: Duration,
        status: std::process::ExitStatus,
    },
    /// Waiting on a spawned child failed. Distinct from `Spawn`: git ran, and
    /// oakum killed it on the way out.
    Wait(io::Error),
    /// Reading a pipe failed partway. Its own variant so a truncated reply is
    /// never handed to a caller as git's whole answer.
    Read(io::Error),
}

impl fmt::Display for RemoteFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "could not run git: {err}"),
            Self::BadDeadline(reason) => f.write_str(reason),
            Self::Wait(err) => write!(
                f,
                "git started but waiting on it failed ({err}); oakum killed it"
            ),
            Self::Read(err) => {
                write!(f, "git ran but its output could not be read: {err}")
            }
            Self::DrainStalled { limit, status } => write!(
                f,
                "git exited ({status}) but something it spawned still held its \
                 output open {}s later, so the answer could not be collected; a \
                 credential helper or an ssh control master is the likely cause. \
                 Set OAKUM_REMOTE_DEADLINE (seconds) to wait longer",
                limit.as_secs()
            ),
            Self::Deadline { limit } => write!(
                f,
                "gave up after {}s with no answer and killed the \
                 child; a credential helper, an interactive \
                 ProxyCommand, or a signing program that prompts can \
                 block past every prompt oakum \
                 suppresses. Set OAKUM_REMOTE_DEADLINE (seconds) \
                 if this remote is legitimately slow",
                limit.as_secs()
            ),
        }
    }
}

/// Generous, because a tag push of a large repository is legitimately slow;
/// the point is bounding the unbounded, not being tight. `OAKUM_REMOTE_DEADLINE`
/// (whole seconds) overrides it — a preference, so it is settable, but an env
/// var rather than a config key: it belongs to the machine and the moment (CI
/// timeout budgets, one slow migration push), not to the repository.
const REMOTE_DEADLINE: Duration = Duration::from_mins(5);

fn remote_deadline() -> Result<Duration, RemoteFailure> {
    match std::env::var_os("OAKUM_REMOTE_DEADLINE") {
        None => Ok(REMOTE_DEADLINE),
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map(Duration::from_secs)
            .ok_or_else(|| {
                RemoteFailure::BadDeadline(format!(
                    "OAKUM_REMOTE_DEADLINE must be a positive whole number of \
                     seconds, got `{}`",
                    value.to_string_lossy()
                ))
            }),
    }
}

impl DeadlinedGit {
    /// Runs the child under the wall-clock deadline.
    ///
    /// The pipes are drained on their own threads so a child writing more
    /// than a pipe buffer cannot deadlock against the timed wait, and the
    /// drained bytes are collected through channels so the wait for them is
    /// bounded by the same deadline: a grandchild (ssh, a helper) inherits
    /// the pipes and can hold them open past the child's own exit, and a
    /// plain join there would re-open the unbounded block the deadline
    /// exists to close. On expiry the drain threads are abandoned.
    ///
    /// # Errors
    ///
    /// The spawn failure, a rejected `OAKUM_REMOTE_DEADLINE`, or the expired
    /// deadline.
    pub(super) fn output(mut self) -> Result<Output, RemoteFailure> {
        let limit = remote_deadline()?;
        self.0
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = self.0.spawn().map_err(RemoteFailure::Spawn)?;
        let stdout = drain(child.stdout.take().expect("stdout was piped"));
        let stderr = drain(child.stderr.take().expect("stderr was piped"));
        let started = std::time::Instant::now();
        let status = loop {
            let waited = match child.try_wait() {
                Ok(waited) => waited,
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RemoteFailure::Wait(err));
                }
            };
            match waited {
                Some(status) => break status,
                None if started.elapsed() >= limit => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RemoteFailure::Deadline { limit });
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        collect_drains(status, stdout, stderr, limit, started)
    }
}

/// Split from [`DeadlinedGit::output`] so `DrainStalled` and `Read` are
/// exercisable with fake receivers in milliseconds.
fn collect_drains(
    status: ExitStatus,
    stdout: Receiver<io::Result<Vec<u8>>>,
    stderr: Receiver<io::Result<Vec<u8>>>,
    limit: Duration,
    started: Instant,
) -> Result<Output, RemoteFailure> {
    let mut streams = Vec::new();
    for drained in [stdout, stderr] {
        // A small floor so a child that exits on the buzzer is not
        // misreported as a stalled drain: its bytes are already queued,
        // and the grace only covers collecting them.
        let remaining = limit
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(50));
        match drained.recv_timeout(remaining) {
            Ok(Ok(bytes)) => streams.push(bytes),
            Ok(Err(err)) => return Err(RemoteFailure::Read(err)),
            Err(_) => return Err(RemoteFailure::DrainStalled { limit, status }),
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

fn drain(
    mut stream: impl io::Read + Send + 'static,
) -> std::sync::mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let _ = sender.send(stream.read_to_end(&mut collected).map(|_| collected));
    });
    receiver
}

/// The base of every git child oakum spawns.
///
/// Sets the working directory and the no-prompt environment together so the
/// transport cannot be resolved against one repository and applied to another.
///
/// Written per site instead, this drifts: `config_probe` below is spawned
/// before `remote_command` and had neither the askpass suppression nor the
/// trace removal until each was noticed separately, and the second cost every
/// remote operation for anyone with tracing on.
pub(super) fn local_command(repo: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "");
    super::untrace(&mut command);
    command
}

/// The transport applied to a child typed as local: git decides for itself
/// when to open a socket — a partial clone's `diff` lazily fetches from the
/// promisor remote — so `BatchMode` must ride every child, not only the ones
/// whose argv names a remote. `local_command` itself stays bare because the
/// transport probe below spawns through it.
pub(super) fn protected_command(repo: &Path, args: &[&str], batch: &BatchSsh) -> Command {
    let mut command = local_command(repo, args);
    if let Some(ssh) = batch.ssh_command() {
        command.env("GIT_SSH_COMMAND", ssh);
    }
    command
}

pub(super) fn deadlined_command(repo: &Path, args: &[&str], batch: &BatchSsh) -> DeadlinedGit {
    DeadlinedGit(protected_command(repo, args, batch))
}

/// Resolves what ssh transport a remote child would use, which is a property of
/// the process environment and the repository config and so cannot change while
/// oakum runs. [`super::Git`] calls this once and reuses the answer; resolving
/// it per child costs an extra `git config` spawn every time — 1 + 2N of them
/// in an N-tag release.
///
/// # Errors
///
/// Ssh configuration oakum cannot read is a failure, never "unset" — but only
/// when the configuration would still be consulted. `GIT_SSH_COMMAND` and
/// `GIT_SSH_VARIANT` outrank every other source; when both are set the config
/// probe is skipped. Guessing a bare `ssh` when the probe fails would replace
/// a key or proxy the user configured. The reason travels bare so the caller
/// decides whether it is fatal; see [`super::Git::unreadable_transport`].
pub(super) fn batch_transport(repo: &Path) -> Result<BatchSsh, String> {
    transport(repo).map(batch_ssh)
}

/// Says a line on stderr, reporting whether it landed. A refused write — a
/// broken pipe, a full disk — reports `false`; a *closed* stderr does not,
/// because `writeln!` to [`io::stderr`] returns `Ok` on `EBADF` (measured on
/// rustc 1.97.1, where the raw `write(2, ..)` beneath it returns -1).
///
/// Whether this has already been said for a remote is [`super::Git`]'s to track:
/// a process-wide `Once` here lets whichever remote resolves first consume the
/// only line, so a mistyped remote silences the warning for a real one.
#[must_use = "a discarded refusal records an unsaid note as said"]
pub(super) fn warn(note: &str) -> bool {
    say(io::stderr(), note)
}

fn say(mut out: impl Write, note: &str) -> bool {
    writeln!(out, "{note}").is_ok()
}

/// What git would use for ssh, in git's own precedence order.
enum SshTransport {
    /// Shell-parsed by git, so options can be appended.
    Composable(String),
    /// Cannot take appended options; the string says why.
    Opaque(String),
    /// Nothing configured; git runs plain `ssh`.
    Default,
}

/// Whether `BatchMode=yes` reached the transport.
#[derive(Debug)]
pub(super) enum BatchSsh {
    Composed(String),
    /// Appended, but the transport already chose `BatchMode` and ssh takes the
    /// first value, so oakum's has no effect.
    Inert {
        ssh: String,
        reason: String,
    },
    /// Nothing could be appended.
    Unprotected(String),
}

impl BatchSsh {
    /// What to put in `GIT_SSH_COMMAND`, when there is anything to put there.
    fn ssh_command(&self) -> Option<&str> {
        match self {
            Self::Composed(ssh) | Self::Inert { ssh, .. } => Some(ssh),
            Self::Unprotected(_) => None,
        }
    }

    /// Why a prompt could still block, when it could. `Composed` is the case
    /// where it cannot, so it has nothing to say.
    pub(super) fn unprotected_reason(&self) -> Option<&str> {
        match self {
            Self::Composed(_) => None,
            Self::Inert { reason, .. } | Self::Unprotected(reason) => Some(reason),
        }
    }
}

fn transport(repo: &Path) -> Result<SshTransport, String> {
    // Environment before config: GIT_SSH_COMMAND / GIT_SSH_VARIANT outrank
    // core.sshCommand / ssh.variant, so an unreadable config must not fail a
    // remote when both env vars already decide the transport (okm-7za.7).
    let env_command = env_value("GIT_SSH_COMMAND")?;
    let env_variant = env_value("GIT_SSH_VARIANT")?;
    let (command, variant) = if let (Some(command), Some(variant)) = (&env_command, &env_variant) {
        (Some(command.clone()), Some(variant.clone()))
    } else {
        let config = config_probe(repo)?;
        (
            env_command.or(config.ssh_command),
            env_variant.or(config.ssh_variant),
        )
    };
    // Checked before the command itself: a `plink` transport is opaque however
    // git was pointed at it.
    if let Some(reason) = opaque_variant(variant.as_deref(), command.as_deref()) {
        return Ok(SshTransport::Opaque(reason));
    }
    if let Some(command) = command {
        return Ok(SshTransport::Composable(command));
    }
    if let Some(program) = env_value("GIT_SSH")? {
        return Ok(SshTransport::Opaque(format!(
            "GIT_SSH names `{program}`, which takes its arguments from git, not from oakum"
        )));
    }
    Ok(SshTransport::Default)
}

/// Git picks a transport's argument style from `ssh.variant`, falling back to
/// the command's basename. Only the OpenSSH styles accept `-o`; injecting it
/// into the others turns a working push into a hard failure, and git drops its
/// own `-o SendEnv=GIT_PROTOCOL` for exactly that reason.
fn opaque_variant(variant: Option<&str>, command: Option<&str>) -> Option<String> {
    const OPAQUE_PROGRAMS: [&str; 3] = ["plink", "putty", "tortoiseplink"];
    if let Some(variant) = variant {
        return (!matches!(variant, "auto" | "ssh"))
            .then(|| format!("ssh.variant is `{variant}`, which does not take `-o` options"));
    }
    let program = command?.split_whitespace().next()?;
    let name = Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())?
        .to_ascii_lowercase();
    OPAQUE_PROGRAMS.contains(&name.as_str()).then(|| {
        format!("`{program}` does not take `-o` options; git treats it as the `{name}` variant")
    })
}

/// Appends unconditionally. Ssh takes the first value of a repeated option, so a
/// user's own `-o BatchMode=no` still wins and a `BatchMode no` in `ssh_config`
/// is still overridden. Deciding by reading the command line would mean parsing
/// it, and a nested `ProxyCommand` carrying `BatchMode=no` reads identically to
/// an outer one while meaning the opposite.
fn batch_ssh(transport: SshTransport) -> BatchSsh {
    match transport {
        SshTransport::Composable(command) => {
            let ssh = format!("{command} -o BatchMode=yes");
            if may_already_set_batch_mode(&command) {
                return BatchSsh::Inert {
                    ssh,
                    reason: format!("`{command}` already sets BatchMode, which ssh reads first"),
                };
            }
            BatchSsh::Composed(ssh)
        }
        SshTransport::Default => BatchSsh::Composed(String::from("ssh -o BatchMode=yes")),
        SshTransport::Opaque(reason) => BatchSsh::Unprotected(reason),
    }
}

/// Deliberately imprecise, and only ever used to decide whether to warn. The
/// same substring test would be unsafe for deciding whether to append — a nested
/// `ProxyCommand` would cancel the option for the outer connection — but a false
/// positive here costs one spurious line while a false negative costs a silent
/// hang.
fn may_already_set_batch_mode(command: &str) -> bool {
    command.to_ascii_lowercase().contains("batchmode")
}

/// # Errors
///
/// A value git would use but oakum cannot read is an error, never "unset".
fn env_value(key: &str) -> Result<Option<String>, String> {
    match std::env::var_os(key) {
        None => Ok(None),
        Some(raw) => raw
            .into_string()
            .map(non_blank)
            .map_err(|_| format!("{key} is not valid UTF-8")),
    }
}

/// Empty is unset. Git runs an empty `GIT_SSH_COMMAND` rather than falling
/// through to `core.sshCommand`, failing with `cannot run : No such file or
/// directory`, and CI templates export unset variables as empty routinely.
fn non_blank(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

/// Both keys in one child, so adding the variant check costs no extra spawn.
struct GitConfig {
    ssh_command: Option<String>,
    ssh_variant: Option<String>,
}

/// An absent key is `None`. A probe that could not run is an error.
fn config_probe(repo: &Path) -> Result<GitConfig, String> {
    // Through the deadline like every other child: this probe is the first
    // spawn of every command, and a `PATH` wrapper or a config include on a
    // hung mount would otherwise block oakum before any operation is named.
    let reply = Reply::from(
        DeadlinedGit(local_command(
            repo,
            &[
                "config",
                "--get-regexp",
                r"^(core\.sshcommand|ssh\.variant)$",
            ],
        ))
        .output()
        .map_err(|failure| failure.to_string())?,
    );
    // git config exits 1 and says nothing when no key matches. A wrapper that
    // exits 1 with a diagnostic failed to look, which is not the same thing.
    if reply.said_no() {
        return Ok(GitConfig {
            ssh_command: None,
            ssh_variant: None,
        });
    }
    if !reply.succeeded() {
        // `detail` rather than stderr alone: a signal leaves both streams empty,
        // and this rendered it as an empty pair of parentheses. Shared with the
        // `Op` path so the two cannot drift apart again.
        return Err(reply.detail());
    }
    let listed =
        String::from_utf8(reply.stdout).map_err(|_| String::from("a value is not valid UTF-8"))?;
    Ok(GitConfig {
        ssh_command: config_value(&listed, "core.sshcommand"),
        ssh_variant: config_value(&listed, "ssh.variant"),
    })
}

/// `--get-regexp` prints `key value` per line. Git resolves a repeated key to
/// the last one, so the last line wins here too.
fn config_value(listed: &str, key: &str) -> Option<String> {
    listed
        .lines()
        .filter_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
        .next_back()
        .and_then(|value| non_blank(value.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    /// A writer that refuses reports failure rather than panicking, so the
    /// caller can decline to record the note as said. Covers the seam, not the
    /// descriptor: [`super::warn`]'s own doc records which `io::stderr`
    /// failures reach it.
    #[test]
    fn a_refused_write_is_reported() {
        struct Full;
        impl std::io::Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut wrote = Vec::new();
        assert!(super::say(&mut wrote, "a note"));
        assert_eq!(wrote, b"a note\n");
        assert!(!super::say(Full, "a note"));
    }

    use super::{
        batch_ssh, config_value, non_blank, opaque_variant, BatchSsh, RemoteFailure, SshTransport,
    };
    use std::time::{Duration, Instant};

    fn composed(transport: SshTransport) -> Option<String> {
        match batch_ssh(transport) {
            BatchSsh::Composed(ssh) | BatchSsh::Inert { ssh, .. } => Some(ssh),
            BatchSsh::Unprotected(_) => None,
        }
    }

    #[test]
    fn a_composable_command_keeps_its_own_arguments() {
        assert_eq!(
            composed(SshTransport::Composable(String::from(
                "ssh -i ~/.ssh/deploy"
            )))
            .as_deref(),
            Some("ssh -i ~/.ssh/deploy -o BatchMode=yes")
        );
    }

    #[test]
    fn nothing_configured_still_gets_batch_mode() {
        assert_eq!(
            composed(SshTransport::Default).as_deref(),
            Some("ssh -o BatchMode=yes")
        );
    }

    /// Ssh resolves the conflict, so oakum appends anyway — but says so, because
    /// this is a case where it knows its own option will not take effect.
    #[test]
    fn an_existing_batchmode_choice_is_appended_past_and_reported() {
        match batch_ssh(SshTransport::Composable(String::from(
            "ssh -o BatchMode=no",
        ))) {
            BatchSsh::Inert { ssh, reason } => {
                assert_eq!(ssh, "ssh -o BatchMode=no -o BatchMode=yes");
                assert!(reason.contains("BatchMode"), "{reason}");
            }
            BatchSsh::Composed(ssh) => panic!("an inert append must be reported, got {ssh}"),
            BatchSsh::Unprotected(reason) => panic!("must still append, got {reason}"),
        }
    }

    /// A nested `ProxyCommand` reads like an outer choice but means the
    /// opposite, so the option is still appended and still takes effect.
    #[test]
    fn a_nested_batchmode_does_not_suppress_the_option() {
        let nested = String::from("ssh -o ProxyCommand='ssh -o BatchMode=no bastion nc %h %p'");
        assert_eq!(
            composed(SshTransport::Composable(nested)).as_deref(),
            Some("ssh -o ProxyCommand='ssh -o BatchMode=no bastion nc %h %p' -o BatchMode=yes")
        );
    }

    /// `Git::child` reads this to decide whether a note is owed, and therefore
    /// whether the remote's URL is worth a child. `Composed` is the one case
    /// that owes nothing.
    #[test]
    fn only_a_transport_that_cannot_take_batch_mode_owes_a_reason() {
        assert_eq!(
            BatchSsh::Composed(String::from("ssh -o BatchMode=yes")).unprotected_reason(),
            None
        );
        assert_eq!(
            BatchSsh::Inert {
                ssh: String::from("ssh -o BatchMode=no"),
                reason: String::from("the transport already chose BatchMode"),
            }
            .unprotected_reason(),
            Some("the transport already chose BatchMode")
        );
        assert_eq!(
            BatchSsh::Unprotected(String::from("ssh.variant is opaque")).unprotected_reason(),
            Some("ssh.variant is opaque")
        );
    }

    #[test]
    fn an_opaque_transport_is_unprotected_and_says_why() {
        match batch_ssh(SshTransport::Opaque(String::from(
            "GIT_SSH names `/x/my-ssh`",
        ))) {
            BatchSsh::Unprotected(reason) => {
                assert!(reason.contains("/x/my-ssh"), "{reason}");
            }
            BatchSsh::Composed(ssh) | BatchSsh::Inert { ssh, .. } => {
                panic!("GIT_SSH cannot take appended options, got {ssh}")
            }
        }
    }

    #[test]
    fn an_explicit_non_ssh_variant_is_opaque() {
        let reason = opaque_variant(Some("simple"), Some("/x/wrapper")).expect("opaque");
        assert!(reason.contains("simple"), "{reason}");
        assert!(opaque_variant(Some("putty"), None).is_some());
    }

    #[test]
    fn the_openssh_variants_still_take_options() {
        assert_eq!(opaque_variant(Some("ssh"), Some("plink")), None);
        assert_eq!(opaque_variant(Some("auto"), Some("plink")), None);
    }

    /// Under `auto` git picks the style from the basename, so oakum must too.
    #[test]
    fn a_plink_basename_is_opaque_under_auto() {
        assert!(opaque_variant(None, Some("C:/PuTTY/plink.exe -batch")).is_some());
        assert!(opaque_variant(None, Some("/usr/bin/tortoiseplink")).is_some());
        assert_eq!(opaque_variant(None, Some("/usr/bin/ssh -i /k")), None);
        assert_eq!(opaque_variant(None, None), None);
    }

    #[test]
    fn config_values_are_read_per_key_and_last_wins() {
        let listed =
            "core.sshcommand ssh -i /first\nssh.variant simple\ncore.sshcommand ssh -i /last\n";
        assert_eq!(
            config_value(listed, "core.sshcommand").as_deref(),
            Some("ssh -i /last")
        );
        assert_eq!(
            config_value(listed, "ssh.variant").as_deref(),
            Some("simple")
        );
        assert_eq!(config_value(listed, "core.askpass"), None);
    }

    #[test]
    fn blank_and_whitespace_values_are_unset() {
        assert_eq!(non_blank(String::new()), None);
        assert_eq!(non_blank(String::from("   ")), None);
        assert_eq!(non_blank(String::from("ssh")).as_deref(), Some("ssh"));
    }

    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        }
    }

    /// A disconnected receiver fails immediately — no wall-clock grace —
    /// and that path is `DrainStalled`, the same as a timed-out drain.
    #[test]
    fn collect_drains_reports_a_stalled_pipe() {
        let (drop_out, stdout) = std::sync::mpsc::channel::<std::io::Result<Vec<u8>>>();
        let (_keep_err, stderr) = std::sync::mpsc::channel();
        drop(drop_out);
        let err = super::collect_drains(
            exit_status(0),
            stdout,
            stderr,
            Duration::from_secs(5),
            Instant::now(),
        )
        .expect_err("a dropped drain is stalled");
        match &err {
            RemoteFailure::DrainStalled { limit, .. } => {
                assert_eq!(*limit, Duration::from_secs(5));
            }
            other => panic!("expected DrainStalled, got {other}"),
        }
        let text = err.to_string();
        assert!(text.contains("still held its output open"), "{text}");
        assert!(text.contains("OAKUM_REMOTE_DEADLINE"), "{text}");
    }

    #[test]
    fn collect_drains_reports_a_read_failure() {
        let (out_tx, stdout) = std::sync::mpsc::channel();
        let (err_tx, stderr) = std::sync::mpsc::channel();
        out_tx.send(Ok(b"ok".to_vec())).expect("stdout queued");
        err_tx
            .send(Err(std::io::Error::other("pipe broke")))
            .expect("stderr queued");
        let err = super::collect_drains(
            exit_status(0),
            stdout,
            stderr,
            Duration::from_secs(5),
            Instant::now(),
        )
        .expect_err("a failed read is Read");
        match err {
            RemoteFailure::Read(ref inner) => {
                assert!(inner.to_string().contains("pipe broke"), "{inner}");
            }
            other => panic!("expected Read, got {other}"),
        }
        assert!(err.to_string().contains("could not be read"), "{}", err);
    }

    #[test]
    fn wait_failure_names_the_kill() {
        let err = RemoteFailure::Wait(std::io::Error::other("wait interrupted"));
        let text = err.to_string();
        assert!(text.contains("waiting on it failed"), "{text}");
        assert!(text.contains("oakum killed it"), "{text}");
        assert!(text.contains("wait interrupted"), "{text}");
    }

    #[test]
    fn deadline_failure_names_the_lever() {
        let err = RemoteFailure::Deadline {
            limit: Duration::from_secs(2),
        };
        let text = err.to_string();
        assert!(text.contains("gave up after 2s"), "{text}");
        assert!(text.contains("OAKUM_REMOTE_DEADLINE"), "{text}");
    }
}
