//! Change files: the changeset-format intersection and directory reading rules.
//!
//! [`format`] is pure string parse/write (ADR-0005 / `okm-ep0`). [`read`] applies
//! the discovery rules from `docs/specs/bump-files.md` (`okm-wnp`): skip list,
//! resolve names against a workspace, skip malformed bodies and continue.
//! Listing `.changeset/` on disk is left to the I/O boundary that owns the path
//! (ADR-0002); this module takes already-loaded `(name, body)` pairs.

mod format;
mod read;

pub use format::{parse, write, ChangeFile, KnopePresence, ParseError, WriteError};
pub use read::{
    is_bump_file_name, load_bump_files, resolve_bump_file, skipped_instruction_name, LoadAbort,
    LoadError, LoadedBumpFiles, MalformedBumpFile, UnknownPackage, UnknownReason,
};
