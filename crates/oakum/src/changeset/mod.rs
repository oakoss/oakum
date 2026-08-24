//! Change files: the changeset-format intersection and directory reading rules.
//!
//! [`format`] is pure string parse/write (ADR-0005 / `okm-ep0`). [`read`] applies
//! the discovery rules from `docs/specs/bump-files.md` (`okm-wnp`): skip list,
//! resolve names against a workspace, skip malformed bodies and continue.
//! [`add`] parses `--packages` and slugifies stems for the binary's write path.
//! Listing `.changeset/` on disk is left to the I/O boundary that owns the path
//! (ADR-0002); this module takes already-loaded `(name, body)` pairs.

mod add;
mod format;
mod read;

pub use add::{default_stem, parse_packages_list, slugify, PackageSpec, PackagesError};
pub use format::{
    parse, parse_migration, write, ChangeFile, KnopePresence, ParseError, WriteError,
};
pub use read::{
    classify_instruction_name, instruction_occupants, is_bump_file_name,
    listing_contains_bump_file, load_bump_files, resolve_bump_file, resolve_package_name,
    skipped_instruction_name, InstructionKind, InstructionOccupant, LoadAbort, LoadError,
    LoadedBumpFiles, MalformedBumpFile, UnknownPackage, UnknownReason,
};
