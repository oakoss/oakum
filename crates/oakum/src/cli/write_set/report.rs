//! What a failed write set left, and how it reads.

use std::fmt;
use std::path::Path;

use super::super::fs::STAGING_CLAIM;
use super::super::printable::Printable;

/// One file the tree kept, and which way. Rendering stays here so a caller can
/// ask what was left without parsing the sentence it prints.
#[derive(Debug)]
pub(in crate::cli) enum LeftChanged {
    /// A restore that reported failure: the file holds neither its original
    /// nor the planned content reliably.
    Unrestored { path: String, err: String },
    /// A deleted file whose rename back failed; `staging` may hold its only copy.
    Stranded {
        path: String,
        staging: String,
        err: String,
    },
    /// A create that landed and whose removal failed.
    Created { path: String },
    /// A create whose outcome could not be checked: the stat failed for a
    /// reason other than the file being absent.
    Unchecked { path: String, err: String },
    /// A staging file this run wrote and could not remove.
    StagingLeak { path: String },
    /// A link the write replaced with a regular file: its original text is
    /// back, so the next `version` may plan over it, but the link is not.
    LinkLost {
        path: String,
        target: Option<String>,
    },
    /// The consume marker, kept because the rollback left something changed,
    /// and why it could not be marked rolled back, if it could not.
    MarkerKept {
        path: String,
        marked: Result<(), String>,
    },
}

impl fmt::Display for LeftChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrestored { path, err } => {
                write!(
                    f,
                    "{} (restore failed: {})",
                    Printable(path),
                    Printable(err)
                )
            }
            Self::Stranded { path, staging, err } => write!(
                f,
                "{} (moved to {}, and could not be moved back: {})",
                Printable(path),
                Printable(staging),
                Printable(err)
            ),
            Self::Created { path } => {
                write!(f, "{} (created and could not be removed)", Printable(path))
            }
            Self::Unchecked { path, err } => write!(
                f,
                "{} (could not check whether this run created it: {}; remove it only if you \
                 know it did)",
                Printable(path),
                Printable(err)
            ),
            Self::StagingLeak { path } => write!(f, "{} ({STAGING_CLAIM})", Printable(path)),
            Self::LinkLost { path, target } => write!(
                f,
                "{} ({} before this run: it holds its original text again, as a regular file, \
                 and the link was not put back)",
                Printable(path),
                Printable(&describe_link(path, target.as_deref()))
            ),
            Self::MarkerKept {
                path,
                marked: Ok(()),
            } => write!(
                f,
                "{} (kept: `oakum version` refuses while it is here; remove it once the other \
                 files listed are put right)",
                Printable(path)
            ),
            // Unmarked, the next `version` reads it as a run whose writes
            // landed, and its advice would remove bump files.
            Self::MarkerKept {
                path,
                marked: Err(err),
            } => write!(
                f,
                "{} (kept, but it could not be marked as rolled back ({}), so the next `oakum \
                 version` will misread it: keep none of this run's writes, remove no bump file, \
                 put the other files listed right, then remove it)",
                Printable(path),
                Printable(err)
            ),
        }
    }
}

/// The link text is relative to the link's own directory, which a reader
/// recreating it from the repository root would otherwise get wrong. Unescaped:
/// both entries that carry it pass it through `Printable`.
pub(super) fn describe_link(path: &str, target: Option<&str>) -> String {
    let Some(target) = target else {
        return String::from("a link");
    };
    match Path::new(path).parent() {
        Some(parent) if !parent.as_os_str().is_empty() && !Path::new(target).has_root() => format!(
            "a link whose text was `{target}`, relative to `{}/`,",
            parent.to_string_lossy()
        ),
        _ => format!("a link whose text was `{target}`"),
    }
}

/// A write set that failed, and everything it could not put back.
#[derive(Debug)]
pub(in crate::cli) enum WriteSetFailure {
    /// Refused before any write was attempted, so there is nothing to put back.
    Refused { cause: String },
    /// Writes were attempted and rolled back. Empty lists mean every restore
    /// reported success and every swept directory could be read, which is
    /// weaker than a byte-identical tree: rollback writes `original` back
    /// without re-reading to confirm it, and owner, inode, and a plan gone
    /// stale since the read are outside the claim.
    RolledBack {
        cause: String,
        left_changed: Vec<LeftChanged>,
        unswept: Vec<String>,
    },
}

impl WriteSetFailure {
    pub(super) fn refused(cause: String) -> Self {
        Self::Refused { cause }
    }

    pub(super) fn rolled_back(
        cause: String,
        mut left_changed: Vec<LeftChanged>,
        mut unswept: Vec<String>,
    ) -> Self {
        // Sorted on the rendered line; ordering by variant would reorder the report.
        left_changed.sort_by_key(ToString::to_string);
        unswept.sort();
        Self::RolledBack {
            cause,
            left_changed,
            unswept,
        }
    }

    /// Each deleted file the rollback could not move back, as `(staging, path)`:
    /// the rename that restores it.
    pub(in crate::cli) fn stranded(&self) -> Vec<(&str, &str)> {
        let Self::RolledBack { left_changed, .. } = self else {
            return Vec::new();
        };
        left_changed
            .iter()
            .filter_map(|entry| match entry {
                LeftChanged::Stranded { path, staging, .. } => {
                    Some((staging.as_str(), path.as_str()))
                }
                _ => None,
            })
            .collect()
    }
}

/// Past this many entries a list stops and says how many it left out, so the
/// advice after it stays on screen.
pub(in crate::cli) const HEAD_FILES: usize = 5;

impl fmt::Display for WriteSetFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (cause, left_changed, unswept) = match self {
            Self::Refused { cause } => return write!(f, "{}", Printable(cause)),
            Self::RolledBack {
                cause,
                left_changed,
                unswept,
            } => (cause, left_changed, unswept),
        };
        write!(f, "{}", Printable(cause))?;
        if !left_changed.is_empty() {
            write!(f, "\n{} file(s) left changed:", left_changed.len())?;
            for entry in left_changed.iter().take(HEAD_FILES) {
                write!(f, "\n  {entry}")?;
            }
            match left_changed.len().saturating_sub(HEAD_FILES) {
                0 => {}
                1 => write!(f, "\n  … 1 more")?,
                rest => write!(f, "\n  … {rest} more")?,
            }
        }
        // Its own list: a directory oakum could not read supports no claim
        // about the tree, and counting it among changed files sends a reader
        // hunting for damage that may not exist.
        if !unswept.is_empty() {
            write!(
                f,
                "\n{} director{} could not be checked for staging files:",
                unswept.len(),
                if unswept.len() == 1 { "y" } else { "ies" }
            )?;
            for entry in unswept {
                write!(f, "\n  {}", Printable(entry))?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for WriteSetFailure {}
