//! The wording every report gives a changed package, so `check`, `status`
//! and the pull-request comment cannot drift apart. Pure: no git, no disk.

use oakum::state::{BumpName, ChangedPackage, CoverSource, PackageRef};

/// ` (via a, b; Cargo.lock: c)` for a change reached through packages oakum
/// does not version, or through a workspace's root files.
pub(in crate::cli) fn via_clause(package: &ChangedPackage) -> String {
    let mut segments = Vec::new();
    if !package.via().is_empty() {
        let names: Vec<&str> = package.via().iter().map(PackageRef::name).collect();
        segments.push(names.join(", "));
    }
    let mut files: Vec<(&str, Vec<&str>)> = Vec::new();
    for through in package.through() {
        match files.iter_mut().find(|(file, _)| *file == through.file) {
            Some((_, names)) => names.push(&through.name),
            None => files.push((&through.file, vec![&through.name])),
        }
    }
    segments.extend(
        files
            .into_iter()
            .map(|(file, names)| format!("{file}: {}", names.join(", "))),
    );
    if segments.is_empty() {
        return String::new();
    }
    format!(" (via {})", segments.join("; "))
}

pub(in crate::cli) fn covered_phrase(package: &ChangedPackage) -> String {
    sources_phrase(package.covered_by().iter()).unwrap_or_default()
}

/// `None` when `sources` is empty.
fn sources_phrase<'a>(sources: impl Iterator<Item = &'a CoverSource>) -> Option<String> {
    let mut parts = Vec::new();
    let mut releaseless = Vec::new();
    for source in sources {
        match source {
            CoverSource::BumpFile { file, bump } => {
                parts.push(format!("`{file}` ({})", bump.as_str()));
            }
            CoverSource::NoneEntry { file } => parts.push(format!("`none` in `{file}`")),
            CoverSource::EmptyFile { file } => {
                parts.push(format!("the empty bump file `{file}`"));
            }
            CoverSource::Commit {
                commit,
                commit_type,
                bump,
            } => {
                let label = commit_label(commit_type.as_deref(), *bump);
                parts.push(match bump {
                    BumpName::Major => format!("commit `{}` ({label})", short(commit)),
                    _ => format!("commit `{}` ({label}, {})", short(commit), bump.as_str()),
                });
            }
            CoverSource::ReleaselessCommit { commit_type, .. } => {
                releaseless.push(commit_type.as_deref().unwrap_or("not conventional"));
            }
        }
    }
    if !releaseless.is_empty() {
        parts.push(format!(
            "commits that release nothing ({})",
            tally(&releaseless)
        ));
    }
    (!parts.is_empty()).then(|| format!("covered by {}", parts.join(" and ")))
}

/// A commit's type as a reason gives it, with `, breaking` for a major bump,
/// or `not conventional` for a message oakum reads as a patch.
fn commit_label(commit_type: Option<&str>, bump: BumpName) -> String {
    match (commit_type, bump) {
        (Some(kind), BumpName::Major) => format!("{kind}, breaking"),
        (Some(kind), _) => kind.to_owned(),
        (None, _) => String::from("not conventional"),
    }
}

/// Why nothing covers a package, beyond that nothing does: the commits asking
/// to release it, which is also why its commits that release nothing do not.
pub(in crate::cli) fn uncovered_reason(package: &ChangedPackage) -> Option<String> {
    reason(package, str::to_owned)
}

/// [`uncovered_reason`] for markdown, where a backtick in a subject would
/// open a code span that runs into the fix after it.
pub(in crate::cli) fn uncovered_reason_markdown(package: &ChangedPackage) -> Option<String> {
    reason(package, |subject| {
        subject.replace('\\', "\\\\").replace('`', "\\`")
    })
}

fn reason(package: &ChangedPackage, subject: impl Fn(&str) -> String) -> Option<String> {
    let commits: Vec<String> = package
        .released_by()
        .iter()
        .map(|commit| {
            format!(
                "`{}` \"{}\" ({})",
                short(&commit.commit),
                subject(&commit.subject),
                commit_label(commit.commit_type.as_deref(), commit.bump)
            )
        })
        .collect();
    let quoted: Vec<&str> = commits.iter().map(String::as_str).collect();
    match quoted.as_slice() {
        [] => None,
        [one] => Some(format!("commit {one} asks for a release")),
        _ => Some(format!(
            "commits {} ask for a release",
            crate::cli::verdict::named(&quoted)
        )),
    }
}

/// How to cover an uncovered package; `generate` is the quoted base when
/// `generate` may be offered.
pub(in crate::cli) fn cover_hint(package: &ChangedPackage, generate: Option<&str>) -> String {
    let none = format!(
        "`oakum add --none --packages {}:none` if it should release nothing",
        package.id().name()
    );
    match (package.fix(), generate) {
        (Some(fix), Some(base)) => format!(
            "add a bump file with `{fix}`, or write it from conventional commits with `oakum generate --from {base}` (or {none})"
        ),
        (Some(fix), None) => format!("add a bump file with `{fix}` (or {none})"),
        (None, _) => String::from(
            "name the package in a conventional commit (or a path that maps to it)",
        ),
    }
}

/// `check`'s stdout account of the look: a count, then one line per way the
/// covered packages are covered, so one bump file naming thirty packages is
/// one line. The uncovered are the refusal's to name.
pub(in crate::cli) fn summary_lines(changed: &[ChangedPackage]) -> Vec<String> {
    if changed.is_empty() {
        return vec![String::from("coverage: no version-managed package changed")];
    }
    let uncovered = changed
        .iter()
        .filter(|package| !package.is_covered())
        .count();
    let mut lines = vec![format!(
        "coverage: {} changed package(s), {} covered, {uncovered} uncovered",
        changed.len(),
        changed.len() - uncovered
    )];
    // An empty bump file covers every changed package, so it is said once
    // rather than beside each of them.
    let mut empty: Vec<&str> = Vec::new();
    for source in changed.iter().flat_map(ChangedPackage::covered_by) {
        if let CoverSource::EmptyFile { file } = source {
            if !empty.contains(&file.as_str()) {
                empty.push(file);
            }
        }
    }
    lines.extend(empty.iter().map(|file| {
        format!("coverage: the empty bump file `{file}` covers every changed package")
    }));
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for package in changed {
        let Some(phrase) = sources_phrase(
            package
                .covered_by()
                .iter()
                .filter(|source| !matches!(source, CoverSource::EmptyFile { .. })),
        ) else {
            continue;
        };
        let label = format!("{}{}", package.id(), via_clause(package));
        match grouped.iter_mut().find(|(seen, _)| *seen == phrase) {
            Some((_, labels)) => labels.push(label),
            None => grouped.push((phrase, vec![label])),
        }
    }
    lines.extend(
        grouped
            .into_iter()
            .map(|(phrase, labels)| format!("coverage: {phrase}: {}", labels.join(", "))),
    );
    lines
}

/// `refactor ×2, test`, in first-seen order.
fn tally(kinds: &[&str]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for kind in kinds {
        match counted.iter_mut().find(|(seen, _)| seen == kind) {
            Some((_, count)) => *count += 1,
            None => counted.push((kind, 1)),
        }
    }
    counted
        .iter()
        .map(|(kind, count)| match count {
            1 => (*kind).to_owned(),
            count => format!("{kind} ×{count}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use oakum::plan::{BumpLevel, Ecosystem, PackageId};
    use oakum::state::ReleasingCommit;

    use super::*;

    fn releasing(
        hash: &str,
        subject: &str,
        kind: Option<&str>,
        level: BumpLevel,
    ) -> ReleasingCommit {
        ReleasingCommit::new(
            hash.to_owned(),
            subject.to_owned(),
            kind.map(str::to_owned),
            level,
        )
        .expect("releases")
    }

    fn uncovered(released_by: Vec<ReleasingCommit>) -> ChangedPackage {
        ChangedPackage::new(
            PackageId::new(Ecosystem::Cargo, "demo"),
            Vec::new(),
            Vec::new(),
            released_by,
            None,
        )
    }

    #[test]
    fn an_uncovered_package_names_each_releasing_commit() {
        let commits = vec![
            releasing("aaaaaaa111", "tweak demo", None, BumpLevel::Patch),
            releasing(
                "bbbbbbb222",
                "feat!: drop x",
                Some("feat"),
                BumpLevel::Major,
            ),
            releasing("ccccccc333", "fix: a bug", Some("fix"), BumpLevel::Patch),
        ];
        assert_eq!(
            uncovered_reason(&uncovered(commits)).as_deref(),
            Some(
                "commits `aaaaaaa` \"tweak demo\" (not conventional), `bbbbbbb` \"feat!: drop x\" (feat, breaking), and `ccccccc` \"fix: a bug\" (fix) ask for a release"
            )
        );
        assert_eq!(uncovered_reason(&uncovered(Vec::new())), None);
    }

    #[test]
    fn a_backtick_in_a_subject_is_escaped_only_for_markdown() {
        let package = uncovered(vec![releasing(
            "aaaaaaa111",
            "fix: odd ` tick",
            Some("fix"),
            BumpLevel::Patch,
        )]);
        assert_eq!(
            uncovered_reason_markdown(&package).as_deref(),
            Some("commit `aaaaaaa` \"fix: odd \\` tick\" (fix) asks for a release")
        );
        assert_eq!(
            uncovered_reason(&package).as_deref(),
            Some("commit `aaaaaaa` \"fix: odd ` tick\" (fix) asks for a release")
        );
        let escaped = uncovered(vec![releasing(
            "aaaaaaa111",
            "fix: path C:\\` tick",
            Some("fix"),
            BumpLevel::Patch,
        )]);
        assert_eq!(
            uncovered_reason_markdown(&escaped).as_deref(),
            Some("commit `aaaaaaa` \"fix: path C:\\\\\\` tick\" (fix) asks for a release"),
            "a backslash before a backtick must not unescape it"
        );
    }

    #[test]
    fn the_hint_offers_none_beside_generate() {
        let package = ChangedPackage::new(
            PackageId::new(Ecosystem::Cargo, "demo"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(String::from("oakum add --packages demo:patch")),
        );
        assert_eq!(
            cover_hint(&package, Some("main")),
            "add a bump file with `oakum add --packages demo:patch`, or write it from conventional commits with `oakum generate --from main` (or `oakum add --none --packages demo:none` if it should release nothing)"
        );
    }

    #[test]
    fn a_covering_commit_says_why_it_releases() {
        let package = ChangedPackage::new(
            PackageId::new(Ecosystem::Cargo, "demo"),
            Vec::new(),
            vec![
                CoverSource::commit(String::from("aaaaaaa111"), None, BumpLevel::Patch),
                CoverSource::commit(
                    String::from("bbbbbbb222"),
                    Some(String::from("feat")),
                    BumpLevel::Major,
                ),
                CoverSource::commit(
                    String::from("ccccccc333"),
                    Some(String::from("feat")),
                    BumpLevel::Minor,
                ),
            ],
            Vec::new(),
            None,
        );
        assert_eq!(
            covered_phrase(&package),
            "covered by commit `aaaaaaa` (not conventional, patch) and commit `bbbbbbb` (feat, breaking) and commit `ccccccc` (feat, minor)"
        );
    }

    /// Each package is named only by what else covers it.
    #[test]
    fn an_empty_bump_file_is_named_once_in_the_account() {
        let id = |name: &str| PackageId::new(Ecosystem::Cargo, name);
        let empty = || CoverSource::EmptyFile {
            file: String::from(".changeset/e.md"),
        };
        let changed = [
            ChangedPackage::new(
                id("alpha"),
                Vec::new(),
                vec![
                    CoverSource::file(String::from(".changeset/a.md"), BumpLevel::Patch),
                    empty(),
                ],
                Vec::new(),
                None,
            ),
            ChangedPackage::new(id("beta"), Vec::new(), vec![empty()], Vec::new(), None),
        ];
        assert_eq!(
            summary_lines(&changed),
            [
                "coverage: 2 changed package(s), 2 covered, 0 uncovered",
                "coverage: the empty bump file `.changeset/e.md` covers every changed package",
                "coverage: covered by `.changeset/a.md` (patch): alpha (cargo)",
            ]
        );
    }
}
