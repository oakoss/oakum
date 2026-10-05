//! The consume journal: the marker `version` holds while it writes and
//! consumes bump files, and the reading of what an interrupted run left.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};

use super::fs::{
    is_consume_staging_name, repo_path_display, staging_candidate, stray_staging_files,
    write_file_via_rename, CONSUME_MARK, STAGING_MARK,
};
use super::version::CHANGESET_DIR;
use super::write_set::read_text;
use super::CliError;

/// Whether a staging name is the marker's own rewrite copy, which only
/// `mark_rolled_back` makes: a kill before its rename leaves it beside a
/// marker that still holds its listing, and the run was a rollback.
pub(super) fn is_marker_rewrite(name: &str) -> bool {
    name.rsplit_once(STAGING_MARK)
        .and_then(|(inner, _)| inner.strip_prefix('.'))
        .is_some_and(is_consume_marker)
}

/// Anything an unfinished consume left that `version` reads to say how to
/// recover. One rule for `check`'s wording and `version`'s refusal, so neither
/// advises removing what the other needs.
pub(super) fn is_consume_leftover(name: &str) -> bool {
    is_consume_staging_name(name) || is_marker_rewrite(name)
}

/// Mark a kept consume marker as rolled back. Falls back to overwriting it in
/// place, which needs no write access to its directory: the fault that
/// strands a file often takes that away.
///
/// # Errors
///
/// Neither the rewrite nor the in-place overwrite landed.
pub(super) fn mark_rolled_back(dir: &Dir, marker: &Path) -> Result<(), CliError> {
    let body = format!("{MARKER_ROLLED_BACK}\n");
    if write_file_via_rename(dir, marker, &body).is_ok() {
        return Ok(());
    }
    let overwrite = dir
        .open_with(marker, OpenOptions::new().write(true))
        .and_then(|mut file| file.write_all(body.as_bytes()));
    overwrite.map_err(|err| {
        CliError::new(format!(
            "failed to mark `{}` as rolled back: {err}",
            repo_path_display(marker)
        ))
    })
}

/// The consume marker's name part, distinct from any bump file's `*.md`.
const CONSUME_MARKER_NAME: &str = "version";

/// Whether a consume staging name is exactly the marker's shape,
/// `.version.oakum-consume.<pid>.<nanos>.<attempt>`: a bump file named
/// `version.oakum-consume.x.md` sets aside to a name that only starts so.
pub(super) fn is_consume_marker(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_prefix(CONSUME_MARKER_NAME))
        .and_then(|rest| rest.strip_prefix(CONSUME_MARK))
        .is_some_and(|tail| {
            let parts: Vec<&str> = tail.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// The marker body a rollback writes when it keeps the marker: it did not put
/// everything back, and no write it made is to be kept.
pub(super) const MARKER_ROLLED_BACK: &str = "rolled-back";

/// One listed path per line, with `\` and a newline escaped: a bump file's
/// name may hold a newline, and a misread listing must not read as complete.
pub(super) fn encode_marker_line(path: &str) -> String {
    path.replace('\\', "\\\\").replace('\n', "\\n")
}

/// `None` for a line `encode_marker_line` could not have written.
pub(super) fn decode_marker_line(line: &str) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                _ => return None,
            }
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

/// Claim the marker a consume holds in `sub` from before its first write
/// until after its last removal, so a run killed anywhere in between leaves a
/// name `version` refuses on. It lists the files the run consumes, one per
/// line, so a later run can tell whether the consume began.
///
/// # Errors
///
/// The marker cannot be created or written; nothing is left behind.
pub(super) fn create_consume_marker(
    dir: &Dir,
    sub: &Path,
    consumes: &[&Path],
) -> Result<PathBuf, CliError> {
    let mut body = String::new();
    for path in consumes {
        body.push_str(&encode_marker_line(&repo_path_display(path)));
        body.push('\n');
    }
    for attempt in 0..16 {
        let candidate = staging_candidate(sub, CONSUME_MARKER_NAME, CONSUME_MARK, attempt);
        match dir.open_with(&candidate, OpenOptions::new().create_new(true).write(true)) {
            Ok(mut file) => {
                return match file.write_all(body.as_bytes()) {
                    Ok(()) => Ok(candidate),
                    Err(err) => {
                        drop(file);
                        let _ = dir.remove_file(&candidate);
                        Err(CliError::new(format!(
                            "failed to write `{}`: {err}",
                            repo_path_display(&candidate)
                        )))
                    }
                };
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(CliError::new(format!(
                    "failed to create `{}`: {err}",
                    repo_path_display(&candidate)
                )));
            }
        }
    }
    Err(CliError::new(format!(
        "failed to create a consume marker in `{}`: every name was taken",
        repo_path_display(sub)
    )))
}

/// What a consume's leftover is, and where the recovery comes from.
pub(super) const CONSUME_CLAIM: &str = "left by an `oakum version` that did not finish; if no \
     oakum run is in progress, run `oakum version` for how to recover";

/// Consume leftovers in `.changeset/` mean a `version` did not finish, and
/// planning again could apply its bump files a second time. Write leftovers
/// are `check`'s to report.
pub(super) fn refuse_interrupted_consume(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    let left: Vec<String> = stray_staging_files(dir, CHANGESET_DIR)?
        .into_iter()
        .filter(|path| is_consume_leftover(file_name(path)))
        .collect();
    if left.is_empty() {
        return Ok(());
    }
    let state = classify(&observe(dir, &left)).advice();
    let mut message = format!(
        "{} file(s) in {CHANGESET_DIR}/ left by an `oakum version` that did not finish; if no \
         oakum run is in progress, {state}:",
        left.len()
    );
    // Debug-quoted: a bump file's name can arrive from a pull request.
    for path in left.iter().take(LEFT_SHOWN) {
        let _ = write!(message, "\n  {path:?}");
    }
    if let Some(rest) = left.len().checked_sub(LEFT_SHOWN).filter(|rest| *rest > 0) {
        let _ = write!(message, "\n  … {rest} more");
    }
    Err(Box::new(CliError::new(message)))
}

/// A kill mid-consume can leave thousands; the advice must stay on screen.
const LEFT_SHOWN: usize = 5;

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// How far an interrupted consume got, read from what it left.
#[derive(Debug, PartialEq, Eq)]
enum ConsumeState {
    /// Every write landed and some bump files were consumed; the rest were
    /// applied by those writes too.
    Landed,
    /// Every write landed and every listed bump file is gone.
    Finished,
    /// No bump file was consumed; the writes may be partial.
    Before,
    /// A marker that cannot be read, or lists nothing it can be checked
    /// against, or a listing no kill could have left. With a set-aside file
    /// present, that file may be a bump's only copy.
    Unknown { set_aside: bool },
    /// A rollback that could not put everything back kept the marker; none
    /// of its writes is to be kept.
    RolledBack,
}

impl ConsumeState {
    /// The recovery `version` prints for this state, after "if no oakum run is
    /// in progress,".
    fn advice(&self) -> &'static str {
        match self {
            Self::RolledBack => {
                "it failed and rolled back, but could not put everything back, and its own error \
                 named what it left. Keep none of its writes: rename each set-aside file back to \
                 its bump-file name, make each manifest and changelog it named match what you \
                 meant, then remove these files"
            }
            Self::Landed => {
                "it had written every manifest and changelog and begun consuming bump files, so the \
                 bump files still beside these were applied too. Keep those writes and remove the \
                 remaining bump files and these files, or revert the writes and rename each \
                 set-aside file back to its bump-file name"
            }
            Self::Finished => {
                "it had written every manifest and changelog and consumed every bump file, so \
                 nothing is owed; remove these files"
            }
            Self::Before => {
                "its manifest and changelog writes may have landed in part or in full, and no bump \
                 file was consumed. Compare the manifest versions and each changelog's top entry \
                 with the bump files, revert any write that landed, then remove these files"
            }
            Self::Unknown { set_aside: false } => {
                "what it left does not say how far it got. Compare the manifest \
                 versions and each changelog's top entry with the bump files to see whether its \
                 writes landed and which bump files they applied, then remove these files"
            }
            Self::Unknown { set_aside: true } => {
                "what it left does not say how far it got, and it set bump files aside. Compare the \
                 manifest versions and each changelog's top entry with the bump files; rename each \
                 set-aside file back to its bump-file name unless the writes you keep applied it, \
                 and remove the marker only once every bump file is accounted for"
            }
        }
    }
}

/// What an interrupted consume left, gathered so [`classify`] needs no I/O.
struct Observed {
    rewrite: bool,
    /// Any leftover that is not a marker: a set-aside bump file, or the
    /// marker's rewrite copy, which `rewrite` decides first.
    set_aside: bool,
    /// In listing order: the first marker that decides wins.
    markers: Vec<MarkerRead>,
}

/// One marker, parsed once, so the paths it lists and the ones still present
/// are counted from the same lines.
enum MarkerRead {
    Unreadable,
    RolledBack,
    Undecodable,
    Listing { listed: usize, present: usize },
}

fn observe(dir: &Dir, left: &[String]) -> Observed {
    let rewrite = left.iter().any(|path| is_marker_rewrite(file_name(path)));
    let set_aside = left.iter().any(|path| !is_consume_marker(file_name(path)));
    let markers = left
        .iter()
        .filter(|path| is_consume_marker(file_name(path)))
        .map(|marker| match read_text(dir, Path::new(marker)) {
            Ok(Some(body)) => read_marker(dir, &body),
            _ => MarkerRead::Unreadable,
        })
        .collect();
    Observed {
        rewrite,
        set_aside,
        markers,
    }
}

fn read_marker(dir: &Dir, body: &str) -> MarkerRead {
    if body.lines().next() == Some(MARKER_ROLLED_BACK) {
        return MarkerRead::RolledBack;
    }
    let (mut listed, mut present) = (0, 0);
    for line in body.lines().filter(|line| !line.is_empty()) {
        let Some(path) = decode_marker_line(line) else {
            return MarkerRead::Undecodable;
        };
        listed += 1;
        if dir.symlink_metadata(path.as_str()).is_ok() {
            present += 1;
        }
    }
    MarkerRead::Listing { listed, present }
}

/// A set-aside file can only follow the last write: bump files are moved
/// aside only once every write landed, and every original goes before any
/// set-aside file is removed. So a listing partly gone with nothing set aside
/// is no state a kill leaves, and reads as unknown.
fn classify(observed: &Observed) -> ConsumeState {
    let set_aside = observed.set_aside;
    if observed.rewrite {
        return ConsumeState::RolledBack;
    }
    let (mut listed, mut present) = (0, 0);
    for marker in &observed.markers {
        match marker {
            MarkerRead::Unreadable | MarkerRead::Undecodable => {
                return ConsumeState::Unknown { set_aside };
            }
            MarkerRead::RolledBack => return ConsumeState::RolledBack,
            MarkerRead::Listing {
                listed: these,
                present: here,
            } => {
                listed += these;
                present += here;
            }
        }
    }
    if set_aside {
        ConsumeState::Landed
    } else if listed == 0 {
        ConsumeState::Unknown { set_aside }
    } else if present == listed {
        ConsumeState::Before
    } else if present == 0 {
        ConsumeState::Finished
    } else {
        ConsumeState::Unknown { set_aside }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{classify, ConsumeState, MarkerRead, Observed};

    #[test]
    fn a_consume_marker_is_told_apart_from_a_set_aside_file() {
        use super::is_consume_marker;
        assert!(is_consume_marker(".version.oakum-consume.1.2.0"));
        assert!(!is_consume_marker(".version.md.oakum-consume.1.2.0"));
        // A bump file named like the marker sets aside to a name that only
        // starts like it.
        assert!(!is_consume_marker(
            ".version.oakum-consume.foo.md.oakum-consume.1.2.0"
        ));
        assert!(!is_consume_marker(".version.oakum-consume.1.2"));
    }

    #[test]
    fn a_marker_line_round_trips_and_rejects_what_it_never_writes() {
        use super::{decode_marker_line, encode_marker_line};
        for path in [
            ".changeset/a.md",
            ".changeset/c\nd.md",
            ".changeset/back\\slash.md",
        ] {
            let line = encode_marker_line(path);
            assert!(!line.contains('\n'), "{line:?}");
            assert_eq!(decode_marker_line(&line).as_deref(), Some(path));
        }
        assert_eq!(decode_marker_line(".changeset/bad\\x.md"), None);
        assert_eq!(decode_marker_line(".changeset/trailing\\"), None);
    }

    #[test]
    fn a_marker_rewrite_copy_is_told_apart() {
        use super::is_marker_rewrite;
        assert!(is_marker_rewrite(
            "..version.oakum-consume.1.2.0.oakum-write.9.8.0"
        ));
        assert!(!is_marker_rewrite("._config.toml.oakum-write.9.8.0"));
        assert!(!is_marker_rewrite(".version.oakum-consume.1.2.0"));
        assert!(!is_marker_rewrite(
            "..a.md.oakum-consume.1.2.0.oakum-write.9.8.0"
        ));
    }

    /// Where the directory refuses new files, the marker is still marked by
    /// overwriting it in place. Unix only: needs a non-root process for the
    /// mode to bite.
    #[cfg(unix)]
    #[test]
    fn a_marker_is_marked_rolled_back_in_a_read_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let root = crate::test_fixture::Fixture::new("consume", "marker-in-place");
        let sub = root.join(".changeset");
        std::fs::create_dir_all(&sub).unwrap();
        let marker = Path::new(".changeset/.version.oakum-consume.1.2.0");
        std::fs::write(root.join(marker), ".changeset/a.md\n.changeset/b.md\n").unwrap();
        let dir = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

        let outcome = super::mark_rolled_back(&dir, marker);

        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(mode)).unwrap();
        outcome.expect("overwritten in place");
        let body = std::fs::read_to_string(root.join(marker)).unwrap();
        assert_eq!(
            body.lines().next(),
            Some(super::MARKER_ROLLED_BACK),
            "{body:?}"
        );
    }

    /// Each rule in the order `classify` applies it; a later case would read
    /// differently if an earlier rule were skipped or moved.
    #[test]
    fn each_leftover_shape_reads_as_its_state() {
        use ConsumeState::{Before, Finished, Landed, RolledBack, Unknown};
        use MarkerRead::{RolledBack as Rolled, Undecodable, Unreadable};
        let listing = |listed, present| MarkerRead::Listing { listed, present };
        let reads = |rewrite, set_aside, markers: Vec<MarkerRead>| {
            classify(&Observed {
                rewrite,
                set_aside,
                markers,
            })
        };
        let unknown = |set_aside| Unknown { set_aside };

        // Beside an unreadable marker, so only a rewrite rule that comes
        // first reads it as rolled back.
        assert_eq!(reads(true, true, vec![Unreadable]), RolledBack, "rewrite");
        assert_eq!(
            reads(false, true, vec![Unreadable]),
            unknown(true),
            "unreadable"
        );
        assert_eq!(
            reads(false, false, vec![Unreadable, Rolled]),
            unknown(false),
            "unreadable first"
        );
        assert_eq!(reads(false, true, vec![Rolled]), RolledBack, "rolled-back");
        assert_eq!(
            reads(false, false, vec![Rolled, Undecodable]),
            RolledBack,
            "rolled-back first"
        );
        assert_eq!(
            reads(false, true, vec![Undecodable]),
            unknown(true),
            "undecodable"
        );
        assert_eq!(
            reads(false, true, vec![]),
            Landed,
            "set-aside-without-marker"
        );
        assert_eq!(reads(false, true, vec![listing(1, 1)]), Landed, "landed");
        assert_eq!(
            reads(false, false, vec![listing(0, 0)]),
            unknown(false),
            "empty listing"
        );
        assert_eq!(
            reads(false, false, vec![listing(2, 2)]),
            Before,
            "all present"
        );
        assert_eq!(
            reads(false, false, vec![listing(1, 0), listing(1, 1)]),
            unknown(false),
            "listings add up"
        );
        assert_eq!(
            reads(false, false, vec![listing(1, 0)]),
            Finished,
            "finished"
        );
        assert_eq!(
            reads(false, false, vec![listing(2, 1)]),
            unknown(false),
            "partly gone"
        );
    }

    /// The lines a marker lists and the ones still present are counted from
    /// one parse, so the two cannot disagree.
    #[test]
    fn a_marker_body_parses_once() {
        let root = crate::test_fixture::Fixture::new("consume", "marker-parse");
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/one.md"), "").unwrap();
        let dir = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let read = |body: &str| super::read_marker(&dir, body);
        assert!(matches!(
            read(".changeset/one.md\n\n.changeset/gone.md\r\n"),
            MarkerRead::Listing {
                listed: 2,
                present: 1
            }
        ));
        assert!(matches!(
            read("rolled-back\n.changeset/bad\\x.md\n"),
            MarkerRead::RolledBack
        ));
        assert!(matches!(
            read(".changeset/one.md\n.changeset/bad\\x.md\n"),
            MarkerRead::Undecodable
        ));
        assert!(matches!(
            read(""),
            MarkerRead::Listing {
                listed: 0,
                present: 0
            }
        ));
    }

    #[test]
    fn each_state_gives_its_own_advice() {
        let says = |state: ConsumeState, phrase: &str| {
            let advice = state.advice();
            assert!(advice.contains(phrase), "{state:?}: {advice}");
            advice
        };
        says(ConsumeState::RolledBack, "Keep none of its writes");
        says(ConsumeState::Landed, "remove the remaining bump files");
        says(ConsumeState::Finished, "nothing is owed");
        says(ConsumeState::Before, "revert any write that landed");
        says(
            ConsumeState::Unknown { set_aside: false },
            "does not say how far it got",
        );
        let unknown_set_aside = says(
            ConsumeState::Unknown { set_aside: true },
            "rename each set-aside file back",
        );
        assert!(
            !unknown_set_aside.contains("then remove these files"),
            "{unknown_set_aside}"
        );
    }
}
