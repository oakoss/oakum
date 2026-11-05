//! What a git child said, and the scripted answers tests stand in for it.

use super::Reads;

/// `None` when a record is not UTF-8. Kept separate from the run so the decoding
/// is testable without a repository.
pub(super) fn split_nul_paths(stdout: &[u8]) -> Option<Vec<String>> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8(record.to_vec()).ok())
        .collect()
}

/// What a git child said.
///
/// `std::process::Output` would do, except that `ExitStatus` has no portable
/// constructor: `ExitStatusExt::from_raw` is a per-target extension trait, so a
/// fake built on it needs a `cfg` branch per platform — the portability problem
/// [`super::fake`] exists to escape. Every rule reads the exit code, so carrying
/// our own leaves them exercisable without a process.
#[derive(Debug)]
pub(in crate::cli) struct Reply {
    /// `None` when a signal killed the child before it could exit.
    pub(super) code: Option<i32>,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl Reply {
    pub(super) fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Whether stdout carries anything the calling accessor would keep, judged
    /// exactly as that accessor judges it. Any disagreement is a hole in one
    /// direction or the other, and both have been live here.
    pub(super) fn spoke(&self, reads: Reads) -> bool {
        match reads {
            Reads::Text => !String::from_utf8_lossy(&self.stdout).trim().is_empty(),
            Reads::Paths => self
                .stdout
                .split(|byte| *byte == 0)
                .any(|record| !record.is_empty()),
        }
    }

    /// Git reports "absent" or "no" as exit 1 with both streams empty. A wrapper
    /// that exits 1 with a diagnostic did not look, which is not the same thing.
    pub(super) fn said_no(&self) -> bool {
        self.code == Some(1) && self.stdout.is_empty() && self.stderr.is_empty()
    }

    /// Git's own words, when it wrote any. A child that failed after explaining
    /// itself on stdout counts: reading stderr alone rendered that as `exit 128
    /// with no diagnostic`, which tells a reader git said nothing and sends
    /// them to inspect a git that did explain itself. Only for a failure, so a
    /// successful op's stdout data never leaks into a message.
    pub(super) fn diagnostic(&self) -> Option<String> {
        let wrote = |bytes: &[u8]| {
            let text = oakum::discover::child::said(bytes);
            (!text.is_empty()).then_some(text)
        };
        wrote(&self.stderr).or_else(|| (!self.succeeded()).then(|| wrote(&self.stdout)).flatten())
    }

    /// The status first, then git's own words when it wrote any: `git push`
    /// writes its success banner to stderr before a signal can kill it, so a
    /// diagnostic alone renders a signal death as that banner (measured).
    pub(super) fn detail(&self) -> String {
        let status = match self.code {
            Some(code) => format!("exit {code}"),
            None => String::from("terminated by a signal"),
        };
        match self.diagnostic() {
            Some(said) => format!("{status}: {said}"),
            None => format!("{status} with no diagnostic"),
        }
    }
}

impl From<std::process::Output> for Reply {
    fn from(output: std::process::Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

/// The answers a test scripts. Named for what git did, not for the exit code,
/// so a script reads as the situation it stands for.
#[cfg(test)]
impl Reply {
    /// Exit 0, having written this.
    pub(in crate::cli) fn said(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// Exit 1 with both streams empty: how git says "no" or "absent".
    pub(in crate::cli) fn absent() -> Self {
        Self {
            code: Some(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// Exit 0 with a warning and no answer — the shape that must not read as an
    /// empty result.
    pub(in crate::cli) fn warned(stderr: &str) -> Self {
        Self {
            code: Some(0),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Exit 0 having answered and warned, which git does: `for-each-ref` lists
    /// the good tags and reports a broken ref on stderr in the same run. The
    /// answer stands.
    pub(in crate::cli) fn said_and_warned(stdout: impl Into<Vec<u8>>, stderr: &str) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Exit non-zero with a diagnostic, which is a failure to look rather than
    /// an answer of "no".
    ///
    /// # Panics
    ///
    /// On a shape that contradicts that: exit 0 is a success, and exit 1 with
    /// nothing written is [`Self::absent`]. Both were constructible, and each
    /// hands the code under test the opposite of what the script says.
    pub(in crate::cli) fn failed(code: i32, stderr: &str) -> Self {
        assert!(
            code != 0,
            "a failure exits non-zero; exit 0 is `said` or `warned`"
        );
        assert!(
            !stderr.is_empty(),
            "a diagnosed failure writes a diagnostic; exit 1 with nothing written is `absent`"
        );
        Self {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A shape with no name: the wrapper behaviour a classifier has to reject,
    /// rather than anything git is known to produce.
    pub(in crate::cli) fn exactly(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    /// Killed before it could exit, so there is no code to read.
    pub(in crate::cli) fn was_signalled() -> Self {
        Self {
            code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}
