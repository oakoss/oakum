//! Git children that contact a remote, built so they do not stop at a prompt.
//!
//! Oakum spawns git with piped standard handles and waits for it, but the child
//! still inherits the controlling terminal. A prompt on `/dev/tty` is an
//! indefinite hang rather than a failure — measured at over 20s against both an
//! https remote without cached credentials and an ssh remote whose host key is
//! unknown. Without a controlling terminal, git already fails on its own.
//!
//! Four sources can prompt. Three are answered here:
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
//! because they are what makes stored credentials authenticate. Only a deadline
//! on the child covers it, along with an interactive `ProxyCommand`, which
//! `BatchMode=yes` does not stop either. Both are okm-e9e.3.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Once;

use super::CliError;

/// A git child that contacts a remote, carrying the environment that keeps it
/// from stopping at a prompt. Opaque so the environment cannot be stripped back
/// off between construction and the run.
pub(super) struct RemoteGit(Command);

impl RemoteGit {
    /// # Errors
    ///
    /// The spawn failure, unchanged.
    pub(super) fn output(mut self) -> io::Result<Output> {
        self.0.output()
    }
}

/// A git child that contacts a remote (`ls-remote`, `push`).
///
/// Sets the working directory and the no-prompt environment together so the
/// transport cannot be resolved against one repository and applied to another.
///
/// Writes one line to stderr, at most once per process, if the transport cannot
/// be given `BatchMode`.
///
/// # Errors
///
/// Ssh configuration oakum cannot read is `unverified`, never "unset":
/// `GIT_SSH_COMMAND` outranks every other source, so guessing would replace the
/// user's key or proxy configuration with bare `ssh`.
pub(super) fn remote_command(repo: &Path, args: &[&str]) -> Result<RemoteGit, CliError> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "");
    match batch_ssh(transport(repo)?) {
        BatchSsh::Composed(ssh) => {
            command.env("GIT_SSH_COMMAND", ssh);
        }
        BatchSsh::Inert { ssh, reason } => {
            command.env("GIT_SSH_COMMAND", ssh);
            warn_once(&reason);
        }
        BatchSsh::Unprotected(reason) => warn_once(&reason),
    }
    Ok(RemoteGit(command))
}

/// The transport is a property of the process environment, so the same line
/// would otherwise print for every remote child — 1 + 2N of them in an N-tag
/// release. Ignores a closed stderr rather than panicking partway through one.
fn warn_once(reason: &str) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        let _ = writeln!(
            io::stderr(),
            "oakum cannot refuse ssh prompts for this transport: {reason}. \
             If this remote reaches git over ssh, a prompt can still block."
        );
    });
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
enum BatchSsh {
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

fn transport(repo: &Path) -> Result<SshTransport, CliError> {
    let config = config_probe(repo)?;
    let command = match env_value("GIT_SSH_COMMAND")? {
        Some(command) => Some(command),
        None => config.ssh_command,
    };
    // Checked before the command itself: a `plink` transport is opaque however
    // git was pointed at it.
    let variant = match env_value("GIT_SSH_VARIANT")? {
        Some(variant) => Some(variant),
        None => config.ssh_variant,
    };
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
/// A value git would use but oakum cannot read is `unverified`, never "unset".
fn env_value(key: &str) -> Result<Option<String>, CliError> {
    match std::env::var_os(key) {
        None => Ok(None),
        Some(raw) => raw.into_string().map(non_blank).map_err(|_| {
            CliError::unverified(format!(
                "unverified: {key} is not valid UTF-8; refusing to replace the ssh transport"
            ))
        }),
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
fn config_probe(repo: &Path) -> Result<GitConfig, CliError> {
    let output = Command::new("git")
        .args([
            "config",
            "--get-regexp",
            r"^(core\.sshcommand|ssh\.variant)$",
        ])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| unreadable(&format!("failed to run git config: {err}")))?;
    // git config exits 1 and says nothing when no key matches. A wrapper that
    // exits 1 with a diagnostic failed to look, which is not the same thing.
    if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
        return Ok(GitConfig {
            ssh_command: None,
            ssh_variant: None,
        });
    }
    if !output.status.success() {
        return Err(unreadable(String::from_utf8_lossy(&output.stderr).trim()));
    }
    let listed =
        String::from_utf8(output.stdout).map_err(|_| unreadable("a value is not valid UTF-8"))?;
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

fn unreadable(detail: &str) -> CliError {
    CliError::unverified(format!(
        "unverified: could not read the ssh configuration ({detail}); \
         refusing to replace the ssh transport"
    ))
}

#[cfg(test)]
mod tests {
    use super::{batch_ssh, config_value, non_blank, opaque_variant, BatchSsh, SshTransport};

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
}
