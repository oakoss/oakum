//! The source tool's config as `migrate` reads it: the settings oakum carries
//! into `.changeset/_config.toml`, and the key names it leaves behind.
//!
//! Both source tools name the one carried setting `privatePackages`, with the
//! independent `version` and `tag` axes oakum spells `private-packages`
//! (ADR-0027), so one reader serves both files. Their schemas differ in one
//! place: changesets also accepts a bare boolean, which its own normalizer
//! expands to both axes, while bumpy's takes an object only. This reader
//! accepts the boolean from either file rather than tracking whose schema
//! permits it — reading a setting the writer plainly meant costs nothing, and
//! refusing it would drop the opt-in this reader exists to carry. Every other
//! key is reported, never guessed at.
//!
//! Both files are read when present, and an axis either opts in. A repository
//! migrating from one tool while a stale config from the other is still on
//! disk therefore carries both, which is the conservative direction: an opt-in
//! that should have lapsed costs a config line, where a dropped one costs a
//! release.
//!
//! One class of fault still refuses, before this module runs: a path that
//! escapes the checkout is rejected by the containment check in
//! [`super::detect_tools`], which inspects these same paths first. That guard
//! is about where a path leads rather than what a file holds, and loosening it
//! for a stale config would loosen it for everything.
//!
//! A file whose `privatePackages` carries one ill-typed axis is skipped whole,
//! so a valid sibling axis goes with it. That is the expensive side of the
//! trade above; the skip line says the file carried nothing, so the loss is
//! visible rather than silent.

use cap_std::fs::Dir;
use oakum::plan::Versioning;

use super::fs::read_text;
use super::owned_files::{ConfigSettings, PrivatePackages};
use super::tag_shape::ReadableTemplate;
use super::CliError;

/// Where a source tool keeps its config, in the order the report names them.
const SOURCE_FILES: [&str; 2] = [".changeset/config.json", ".bumpy/_config.json"];

/// The source key both tools spell alike, mapping onto oakum's
/// `private-packages` (ADR-0027).
const PRIVATE_PACKAGES: &str = "privatePackages";
/// bumpy's name for what oakum calls `commit-message`. An exact equivalent, so
/// it carries rather than being left behind for the reader to restore by hand.
const VERSION_COMMIT_MESSAGE: &str = "versionCommitMessage";

/// What a source tool's `versionCommitMessage` turned out to be. An `Option`
/// reported all four outcomes with the same sentence, which for a value equal
/// to oakum's default reported a loss that did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CarriedMessage {
    /// The source file states none — or states a non-string, which the
    /// dropped-key line reports instead.
    Absent,
    /// Stated, and identical to what `ci version-pr` writes anyway. Writing it
    /// would be a config line restating a default ([ADR-0004]); saying nothing
    /// would report a loss that did not happen.
    ///
    /// [ADR-0004]: ../../../../docs/decisions/0004-derive-facts-configure-preference.md
    SameAsDefault,
    /// Stated, and oakum will not write it.
    Unwritable(Refusal),
    /// Stated, and carried into `commit-message`.
    Carried(String),
}

/// Why oakum will not write a stated message. Closed, so a new reason is a
/// compile-checked addition rather than another sentence fragment — one of the
/// three this replaced claimed a tab cannot sit in a TOML basic string, which is
/// measurably false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Empty,
    WhitespaceOnly,
    /// A newline breaks the config's string form outright; the rest make a
    /// commit headline no reader wants.
    ControlCharacter,
    /// `commit-message` is rendered, not written literally: measured, `{{ … }}`
    /// fails the version PR with an undefined value, and `{% … %}` renders to a
    /// different message without failing at all.
    TemplateSyntax,
    /// bumpy takes this key "as string or module path"
    /// (`docs/research/bump-file-tool-interfaces.md`). A path is code bumpy
    /// runs; oakum runs none ([ADR-0006]), so carrying it verbatim would make
    /// the path itself the commit headline — a silent mistranslation rather
    /// than a carry.
    ///
    /// [ADR-0006]: ../../../../docs/decisions/0006-no-command-execution-in-templates.md
    ModulePath,
}

impl Refusal {
    /// Completes "oakum could not carry it because …".
    fn because(self) -> &'static str {
        match self {
            Self::Empty => "it states an empty message",
            Self::WhitespaceOnly => {
                "it is only whitespace, and `ci version-pr` refuses a message that renders to nothing"
            }
            Self::ControlCharacter => {
                "it holds a control character, which no commit headline should carry"
            }
            Self::TemplateSyntax => {
                "it holds template syntax, and `commit-message` is rendered as a template rather than written literally"
            }
            Self::ModulePath => {
                "it names a module rather than a message, and oakum runs no such module"
            }
        }
    }

    /// An empty message is nothing to put back, so it is reported rather than
    /// handed to the reader as work.
    fn owed_to_the_reader(self) -> bool {
        self != Self::Empty
    }
}

/// One source config file, read once: what oakum carries out of it and what
/// it does not.
#[derive(Debug)]
pub(super) struct SourceConfig {
    /// Repository-relative, and still on disk after migrate: the report says so.
    pub(super) file: &'static str,
    /// Top-level key names with no oakum counterpart, sorted.
    pub(super) dropped: Vec<String>,
    commit_message: CarriedMessage,
    /// `None` when the file did not set `privatePackages`.
    private_packages: Option<PrivatePackages>,
}

impl SourceConfig {
    /// The carried setting, only when it differs from oakum's default. Both
    /// axes off is what an absent `[private-packages]` already means, so
    /// there is nothing for the report to announce.
    pub(super) fn carried_private_packages(&self) -> Option<PrivatePackages> {
        self.private_packages.filter(|private| private.any())
    }
}

/// Every source config present in the repository, and a line for each one that
/// could not be used. Both tools can be detected at once; each file speaks for
/// itself.
///
/// A source config this function cannot open, read, or parse is reported and
/// skipped rather than refused. Both source tools load their config through
/// JSON5-tolerant readers, so a `//` note or a trailing comma is a file they
/// accept and `serde_json` does not. A path that escapes the checkout never
/// reaches here; see the module docs.
///
/// The report says what was skipped and what the skip may have cost, so a
/// dropped opt-in is visible rather than assumed absent, and `check` refuses
/// afterwards if the result manages nothing — the two halves cover each other.
pub(super) fn read_source_configs(dir: &Dir) -> (Vec<SourceConfig>, Vec<String>) {
    let mut configs = Vec::new();
    let mut unreadable = Vec::new();
    for file in SOURCE_FILES {
        // A directory or an unreadable mode at this path is no more the
        // migration's business than a trailing comma inside it.
        let outcome = match read_text(dir, file) {
            Ok(None) => continue,
            Ok(Some(body)) => parse_source_config(file, &body),
            Err(err) => Err(err),
        };
        match outcome {
            Ok(config) => configs.push(config),
            Err(err) => unreadable.push(format!(
                "could not use `{file}` ({err}); no settings carried from it, \
                 including `{PRIVATE_PACKAGES}` if it set one"
            )),
        }
    }
    (configs, unreadable)
}

/// What `migrate` writes into `_config.toml`: both intent mechanisms on, the
/// versioning it inferred, the source tools' carried settings, and the tag
/// shape derived from the repository's own history rather than from any of
/// them — neither source tool records one.
pub(super) fn migrated_settings(
    versioning: Versioning,
    configs: &[SourceConfig],
    tag_format: Option<ReadableTemplate>,
) -> ConfigSettings {
    ConfigSettings {
        change_files: true,
        conventional_commits: true,
        versioning,
        private_packages: carried_private_packages(configs),
        tag_format,
        commit_message: carried_commit_message(configs),
    }
}

/// Source keys whose nearest oakum setting exists but does not mean the same
/// thing. Named as a step rather than carried, because writing one from the
/// other would silently change what the reader gets.
///
/// `changelog` means different things in each source tool and neither is
/// `template`: bumpy's `['github', {internalAuthors: [...]}]` appends PR and
/// author links and suppresses them for the listed maintainers, and changesets'
/// is a module path to code that runs. oakum executes no such module
/// ([ADR-0006]), so neither converts — which is why the step asks rather than
/// asserting what the key did.
///
/// `versionCommitMessage` is deliberately absent: it maps exactly, so it is
/// carried instead.
///
/// [ADR-0006]: ../../../../docs/decisions/0006-no-command-execution-in-templates.md
const LOSSY_MAPPINGS: [(&str, &str); 1] = [("changelog", "template")];

/// Source keys oakum has no setting for at all, and whose absence rewrites an
/// outcome the reader would otherwise meet after the next release rather than
/// during the cutover. The generic dropped-key line is right for a key nobody
/// misses; these are the ones that change something quietly.
///
/// `gitUser` decided who authored every release commit and tag. oakum has no
/// counterpart and the printed workflow commits as whatever identity matches
/// its token, so a cutover silently reassigns authorship.
const CONSEQUENTIAL_DROPS: [(&str, &str); 1] = [(
    "gitUser",
    "it set who authored release commits and tags; oakum has no identity key. `git config user.name` / `user.email` in that job decides the tagger; the version commit is written through the GitHub API and carries the token's own account, which no git config can change",
)];

/// The remaining step for a dropped key whose absence changes an outcome.
/// Separate from [`lossy_mapping_steps`]: there is no near-equivalent to decide
/// between, only a consequence to name.
pub(super) fn consequential_drop_steps(configs: &[SourceConfig]) -> Vec<String> {
    let mut steps = Vec::new();
    for config in configs {
        for (source_key, consequence) in CONSEQUENTIAL_DROPS {
            if config.dropped.iter().any(|key| key == source_key) {
                steps.push(format!(
                    "- `{source_key}` from `{}` was not carried over: {consequence}",
                    config.file
                ));
            }
        }
    }
    steps
}

/// The remaining step for a source key oakum has a near-equivalent for. The
/// generic "not carried over" line stays deliberately silent about counterparts;
/// this names only the ones where a counterpart exists and differs, so the
/// reader knows there is a decision to make rather than a key to forget.
pub(super) fn lossy_mapping_steps(configs: &[SourceConfig]) -> Vec<String> {
    let mut steps = Vec::new();
    for config in configs {
        for (source_key, oakum_key) in LOSSY_MAPPINGS {
            if config.dropped.iter().any(|key| key == source_key) {
                steps.push(format!(
                    "- decide what `{source_key}` from `{}` should become: oakum's nearest setting is `{oakum_key}`, which does not mean the same thing, so oakum wrote neither",
                    config.file
                ));
            }
        }
    }
    steps
}

/// The message oakum writes: the first one a source file states that it can
/// carry. Two source tools rarely both state one, and a repository migrating
/// from two is already told about every key the other left behind.
fn carried_commit_message(configs: &[SourceConfig]) -> Option<String> {
    chosen_commit_message(configs).map(|(_, message)| message.to_owned())
}

/// The remaining step for a stated message oakum did not write, and the note for
/// one it did not need to. `Absent` says nothing: there was nothing to carry.
pub(super) fn commit_message_steps(configs: &[SourceConfig]) -> Vec<String> {
    configs
        .iter()
        .filter_map(|config| match &config.commit_message {
            CarriedMessage::Absent | CarriedMessage::Carried(_) => None,
            CarriedMessage::SameAsDefault if carried_commit_message(configs).is_none() => {
                Some(format!(
                    "- `{VERSION_COMMIT_MESSAGE}` in `{}` is what oakum writes anyway, so no `commit-message` line was needed",
                    config.file
                ))
            }
            // A custom message from another file wins, so oakum does not write
            // this one after all.
            CarriedMessage::SameAsDefault => Some(format!(
                "- `{VERSION_COMMIT_MESSAGE}` in `{}` was not carried: another source file states one oakum wrote instead",
                config.file
            )),
            // Winner-aware like its siblings: telling a reader to restore this by
            // hand, into a config that already holds a different carried message,
            // invites them to replace one silently.
            CarriedMessage::Unwritable(refusal) if refusal.owed_to_the_reader() => {
                Some(match chosen_commit_message(configs) {
                    Some((chosen, _)) => format!(
                        "- restore `{VERSION_COMMIT_MESSAGE}` from `{}` by hand only if you want it instead of the one oakum wrote from `{chosen}`: oakum could not carry it because {}",
                        config.file,
                        refusal.because()
                    ),
                    None => format!(
                        "- restore `{VERSION_COMMIT_MESSAGE}` from `{}` by hand: oakum could not carry it because {}",
                        config.file,
                        refusal.because()
                    ),
                })
            }
            CarriedMessage::Unwritable(refusal) => Some(format!(
                "- `{VERSION_COMMIT_MESSAGE}` in `{}` was not carried: {}",
                config.file,
                refusal.because()
            )),
        })
        .collect()
}

/// The one message that reaches the config, and which file stated it.
///
/// Resolved once for the repository rather than per file: the write is
/// first-wins, so announcing every `Carried` value told a reader that two
/// messages were written when one was — measured with a `.changeset/` and a
/// `.bumpy/` config stating different ones.
pub(super) fn chosen_commit_message(configs: &[SourceConfig]) -> Option<(&'static str, &str)> {
    configs
        .iter()
        .find_map(|config| match &config.commit_message {
            CarriedMessage::Carried(message) => Some((config.file, message.as_str())),
            _ => None,
        })
}

/// A message a second source file stated and oakum did not write. Reported so
/// the loser is not lost silently: it is removed from the dropped-key list, so
/// no other line mentions it.
pub(super) fn shadowed_commit_messages(configs: &[SourceConfig]) -> Vec<String> {
    let mut steps = Vec::new();
    let mut winner = None;
    for config in configs {
        let CarriedMessage::Carried(_) = &config.commit_message else {
            continue;
        };
        match winner {
            None => winner = Some(config.file),
            Some(chosen) => steps.push(format!(
                "- `{VERSION_COMMIT_MESSAGE}` in `{}` was not carried: `{chosen}` states one too, and oakum writes a single `commit-message`",
                config.file
            )),
        }
    }
    steps
}

/// Whether the value names code rather than stating a message. A commit
/// headline does not begin `./` and does not end in a module suffix, so the
/// shapes bumpy accepts as a path are the shapes this refuses.
fn looks_like_a_module_path(message: &str) -> bool {
    message.starts_with("./")
        || message.starts_with("../")
        || [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"]
            .iter()
            .any(|suffix| message.ends_with(suffix))
}

/// What the source file states, classified once.
fn read_commit_message(object: &serde_json::Map<String, serde_json::Value>) -> CarriedMessage {
    let Some(message) = object
        .get(VERSION_COMMIT_MESSAGE)
        .and_then(serde_json::Value::as_str)
    else {
        return CarriedMessage::Absent;
    };
    // Classified and carried in trimmed form: `ci version-pr` trims
    // before rendering, so padding would make the commit oakum writes differ
    // from the message migrate announced — and the default plus one trailing
    // space would slip the equality test below and write a line restating it.
    let trimmed = message.trim();
    if trimmed == super::ci::DEFAULT_COMMIT {
        return CarriedMessage::SameAsDefault;
    }
    if let Some(refusal) = refuses_carrying(message, trimmed) {
        return CarriedMessage::Unwritable(refusal);
    }
    CarriedMessage::Carried(trimmed.to_owned())
}

/// Why oakum will not write this message, if it will not.
///
/// Two different destinations have to accept it. The config file is TOML, where
/// a newline ends a basic string outright and the other control characters make
/// a commit headline no reader wants — a tab does sit in a basic string.
/// `commit-message` is then a minijinja template, not a literal: measured, a
/// carried `{{version}}` fails to render at `ci version-pr` with "undefined
/// value", `{{` fails with a syntax error, and `{% … %}` and `{# … #}` render
/// to a *different message* without failing at all. A source tool's literal is
/// not a template, and guessing which braces the author meant literally is not
/// oakum's call.
fn refuses_carrying(stated: &str, trimmed: &str) -> Option<Refusal> {
    if stated.is_empty() {
        return Some(Refusal::Empty);
    }
    if trimmed.is_empty() {
        return Some(Refusal::WhitespaceOnly);
    }
    if trimmed.chars().any(char::is_control) {
        return Some(Refusal::ControlCharacter);
    }
    // Every delimiter minijinja opens with. `template::render` builds a fresh
    // `Environment` with no `set_syntax`, so these are the ones that run.
    if ["{{", "{%", "{#"]
        .iter()
        .any(|sigil| trimmed.contains(sigil))
    {
        return Some(Refusal::TemplateSyntax);
    }
    if looks_like_a_module_path(trimmed) {
        return Some(Refusal::ModulePath);
    }
    None
}

/// On if any source file turned that axis on. Two source tools that disagree
/// would each have opted their own packages in, and oakum has one config for
/// the repository.
pub(super) fn carried_private_packages(configs: &[SourceConfig]) -> PrivatePackages {
    configs
        .iter()
        .filter_map(|config| config.private_packages)
        .fold(PrivatePackages::default(), PrivatePackages::union)
}

fn parse_source_config(
    file: &'static str,
    body: &str,
) -> Result<SourceConfig, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|err| CliError::new(format!("`{file}` is not valid JSON: {err}")))?;
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(format!(
            "`{file}` is not a JSON object"
        ))));
    };
    let private_packages = object
        .get(PRIVATE_PACKAGES)
        .map(|value| parse_private_packages(file, value))
        .transpose()?;
    let commit_message = read_commit_message(object);
    // `Absent` is the only outcome the generic dropped-key line describes: the
    // other three are reported by name, so repeating them there would tell the
    // reader twice, once wrongly.
    let carried: &[&str] = if commit_message == CarriedMessage::Absent {
        &[PRIVATE_PACKAGES]
    } else {
        &[PRIVATE_PACKAGES, VERSION_COMMIT_MESSAGE]
    };
    let mut dropped: Vec<String> = object
        .keys()
        .filter(|key| !carried.contains(&key.as_str()))
        .cloned()
        .collect();
    dropped.sort();
    Ok(SourceConfig {
        file,
        dropped,
        commit_message,
        private_packages,
    })
}

fn parse_private_packages(
    file: &'static str,
    value: &serde_json::Value,
) -> Result<PrivatePackages, Box<dyn std::error::Error>> {
    // Changesets accepts a bare boolean here and expands it to both axes; its
    // schema is `object | boolean` and its normalizer reads a missing value as
    // false.
    if let serde_json::Value::Bool(on) = value {
        return Ok(PrivatePackages {
            version: *on,
            tag: *on,
        });
    }
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(format!(
            "`{PRIVATE_PACKAGES}` in `{file}` is neither an object nor a boolean"
        ))));
    };
    Ok(PrivatePackages {
        version: axis(file, object, "version")?,
        tag: axis(file, object, "tag")?,
    })
}

fn axis(
    file: &'static str,
    object: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    // A missing key is the source tool's own default; an explicit `null` is a
    // value that is not a boolean, and migrating it as `false` would turn a
    // malformed setting into a silent opt-out.
    match object.get(name) {
        None => Ok(false),
        Some(serde_json::Value::Bool(on)) => Ok(*on),
        Some(other) => Err(Box::new(CliError::new(format!(
            "`{PRIVATE_PACKAGES}.{name}` in `{file}` is `{other}`, not a boolean"
        )))),
    }
}

#[cfg(test)]
mod reading {
    use super::{carried_private_packages, parse_source_config};
    use crate::cli::owned_files::PrivatePackages;

    fn parse(file: &'static str, body: &str) -> super::SourceConfig {
        parse_source_config(file, body).expect("source config")
    }

    #[test]
    fn private_packages_is_carried_and_never_reported_as_dropped() {
        let config = parse(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": true, "tag": true}, "baseBranch": "main"}"#,
        );
        assert_eq!(
            config.carried_private_packages(),
            Some(PrivatePackages {
                version: true,
                tag: true
            })
        );
        assert_eq!(config.dropped, vec![String::from("baseBranch")]);
    }

    #[test]
    fn axes_are_independent_and_a_missing_axis_is_off() {
        let config = parse(
            ".changeset/config.json",
            r#"{"privatePackages": {"tag": true}}"#,
        );
        assert_eq!(
            config.carried_private_packages(),
            Some(PrivatePackages {
                version: false,
                tag: true
            })
        );
    }

    #[test]
    fn both_axes_off_matches_the_default_so_nothing_is_announced() {
        let config = parse(
            ".changeset/config.json",
            r#"{"privatePackages": {"version": false, "tag": false}}"#,
        );
        assert_eq!(config.carried_private_packages(), None);
        assert!(config.dropped.is_empty());
    }

    #[test]
    fn a_source_without_the_key_carries_nothing() {
        let config = parse(".changeset/config.json", r#"{"access": "public"}"#);
        assert_eq!(config.carried_private_packages(), None);
        assert_eq!(config.dropped, vec![String::from("access")]);
        assert_eq!(
            carried_private_packages(&[config]),
            PrivatePackages::default()
        );
    }

    #[test]
    fn two_sources_union_their_axes() {
        let carried = carried_private_packages(&[
            parse(
                ".changeset/config.json",
                r#"{"privatePackages": {"version": true}}"#,
            ),
            parse(
                ".bumpy/_config.json",
                r#"{"privatePackages": {"tag": true}}"#,
            ),
        ]);
        assert_eq!(
            carried,
            PrivatePackages {
                version: true,
                tag: true
            }
        );
    }

    #[test]
    fn a_non_boolean_axis_refuses_and_names_the_file() {
        let err = parse_source_config(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": "yes"}}"#,
        )
        .expect_err("non-boolean axis");
        assert_eq!(
            err.to_string(),
            "`privatePackages.version` in `.bumpy/_config.json` is `\"yes\"`, not a boolean"
        );
    }

    #[test]
    fn a_private_packages_that_is_neither_object_nor_boolean_refuses() {
        let err = parse_source_config(".bumpy/_config.json", r#"{"privatePackages": "yes"}"#)
            .expect_err("a string is neither");
        assert_eq!(
            err.to_string(),
            "`privatePackages` in `.bumpy/_config.json` is neither an object nor a boolean"
        );
    }

    /// Changesets' own normalizer expands a bare boolean to both axes.
    #[test]
    fn a_changesets_boolean_private_packages_opts_both_axes_in() {
        let carried = carried_private_packages(&[parse(
            ".changeset/config.json",
            r#"{"privatePackages": true}"#,
        )]);
        assert_eq!(
            carried,
            PrivatePackages {
                version: true,
                tag: true
            }
        );
        let off = carried_private_packages(&[parse(
            ".changeset/config.json",
            r#"{"privatePackages": false}"#,
        )]);
        assert_eq!(off, PrivatePackages::default());
    }

    #[test]
    fn a_non_object_config_refuses_and_names_the_file() {
        let err = parse_source_config(".bumpy/_config.json", "[]").expect_err("array");
        assert_eq!(
            err.to_string(),
            "`.bumpy/_config.json` is not a JSON object"
        );
        let err = parse_source_config(".bumpy/_config.json", "{").expect_err("truncated");
        assert!(
            err.to_string()
                .starts_with("`.bumpy/_config.json` is not valid JSON: "),
            "{err}"
        );
    }
    #[test]
    fn an_explicit_null_axis_refuses_like_any_other_non_boolean() {
        let err = parse_source_config(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": null}}"#,
        )
        .expect_err("null is not a boolean");
        assert!(err.to_string().contains("not a boolean"), "{err}");
    }

    /// bumpy's `gitUser` decided who authored every release commit and tag.
    /// oakum has no counterpart, so a cutover reassigns authorship silently
    /// unless the step says so — `check` cannot see this, and the reader only
    /// meets it after the next release (`okm-404.10`).
    #[test]
    fn git_user_is_named_as_a_consequential_drop_not_only_a_dropped_key() {
        let config = super::SourceConfig {
            file: ".bumpy/_config.json",
            dropped: vec![String::from("baseBranch"), String::from("gitUser")],
            commit_message: super::CarriedMessage::Absent,
            private_packages: None,
        };
        let steps = super::consequential_drop_steps(std::slice::from_ref(&config));
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert!(steps[0].contains("`gitUser`"), "{steps:?}");
        assert!(
            steps[0].contains("who authored release commits"),
            "{steps:?}"
        );
        // `baseBranch` has no consequence to name, so it stays on the generic
        // dropped-key line alone.
        assert!(!steps[0].contains("baseBranch"), "{steps:?}");

        let quiet = super::SourceConfig {
            file: ".bumpy/_config.json",
            dropped: vec![String::from("baseBranch")],
            commit_message: super::CarriedMessage::Absent,
            private_packages: None,
        };
        assert!(super::consequential_drop_steps(&[quiet]).is_empty());
    }
}
