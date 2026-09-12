//! User-owned templates: they render, they do not execute (ADR-0006).
//!
//! Two sources, and only two: an inline string, and `{ file = "path" }`.
//! `{ command = "..." }` is refused at parse. File paths are resolved by the
//! CLI at one containment chokepoint; this module only renders a body it is
//! given.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use minijinja::{Environment, UndefinedBehavior};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

/// Where a template body comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateSource {
    /// The config string is the template itself.
    Inline(String),
    /// Untrusted path from config. Only the CLI load chokepoint may open it.
    File(String),
}

impl<'de> Deserialize<'de> for TemplateSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TemplateVisitor;

        impl<'de> Visitor<'de> for TemplateVisitor {
            type Value = TemplateSource;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a template string or a table `{ file = \"path\" }`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(TemplateSource::Inline(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(TemplateSource::Inline(value))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut file = None;
                let mut extra = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value: String = map.next_value()?;
                    match key.as_str() {
                        "file" => file = Some(value),
                        other => {
                            extra.insert(other.to_owned(), value);
                        }
                    }
                }
                if extra.contains_key("command") {
                    return Err(de::Error::custom(
                        "templates render; they do not execute (ADR-0006)",
                    ));
                }
                if !extra.is_empty() {
                    let keys: Vec<_> = extra.keys().map(String::as_str).collect();
                    return Err(de::Error::custom(format!(
                        "unknown template table key `{}`; only `file` is allowed",
                        keys[0]
                    )));
                }
                let Some(path) = file else {
                    return Err(de::Error::custom(
                        "template table needs `file`; inline templates are a bare string",
                    ));
                };
                if path.trim().is_empty() {
                    return Err(de::Error::custom("`file` is empty"));
                }
                Ok(TemplateSource::File(path))
            }
        }

        deserializer.deserialize_any(TemplateVisitor)
    }
}

/// Why rendering failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderError {
    message: String,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RenderError {}

/// Render `source` against `context`.
///
/// Undefined values are errors except in `{% if %}`, which treats them as
/// false (`UndefinedBehavior::SemiStrict`). `{% include %}` has no loader,
/// so it fails rather than reading the filesystem from here.
///
/// # Errors
///
/// Parse errors, undefined prints, and include/load attempts.
pub fn render(name: &str, source: &str, context: impl Serialize) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    env.add_template(name, source)
        .map_err(|err| render_error(&err))?;
    let template = env.get_template(name).map_err(|err| render_error(&err))?;
    let context = minijinja::Value::from_serialize(context);
    template.render(&context).map_err(|err| {
        let mut failure = render_error(&err);
        if err.kind() == minijinja::ErrorKind::UndefinedError {
            let ambient: BTreeSet<String> =
                env.globals().map(|(name, _)| name.to_owned()).collect();
            failure.message.push_str(": ");
            failure
                .message
                .push_str(&undefined_detail(&template, &context, &ambient));
        }
        failure
    })
}

/// What minijinja will not say. It reports `undefined value` with the template
/// name and line and nothing more, which leaves a reader who wrote
/// `{{ name }}` no way to learn that the variable is spelled `package` short of
/// reading oakum's source — measured during the claude-plugins cutover.
///
/// Both halves are derived: the unsupplied names are the template's own
/// undeclared variables minus what the context supplies and minus the
/// renderer's globals, and the scope is the context's own keys. A hand-written
/// list would be a second declaration of the state each surface serializes.
///
/// Stated as a set rather than as the cause, because that is what it is: the
/// analysis reports every name the template reads anywhere, and cannot say
/// which branch ran. A name inside `{% if false %}` is read by the template and
/// genuinely unsupplied; calling it the reason this render failed would not be.
fn undefined_detail(
    template: &minijinja::Template<'_, '_>,
    context: &minijinja::Value,
    ambient: &BTreeSet<String>,
) -> String {
    let Some(provided) = context_keys(context) else {
        return String::from("oakum could not list the variables this template receives");
    };
    // Globals are subtracted but never listed as received: the analysis counts
    // every name the template did not bind itself, so a `range()` call arrives
    // here looking exactly like a missing variable. Measured on minijinja
    // 2.24: `{% for i in range(3) %}{{ nope }}{% endfor %}` reports
    // `{"range", "nope"}`.
    let mut unsupplied: Vec<String> = template
        .undeclared_variables(true)
        .into_iter()
        // Resolved against the context rather than matched against its top-level
        // keys: `repo.url` is supplied and `repo.nope` is not, and subtracting
        // on the root alone conflated them — the first was accused of being
        // missing, then the second stopped naming itself at all.
        .filter(|name| !resolves(context, name) && !resolves_ambient(ambient, name))
        .collect();
    unsupplied.sort();
    let reads = if unsupplied.is_empty() {
        String::from("a variable this template reads is not defined here")
    } else {
        format!(
            "this template reads {}, which the context does not supply",
            quoted(&unsupplied)
        )
    };
    if provided.is_empty() {
        return format!("{reads}; it receives no variables");
    }
    format!("{reads}; it receives {}", quoted(&provided))
}

/// Whether the context actually supplies this name, dotted reads included.
/// Walked a segment at a time: minijinja's own path lookup is private.
fn resolves(context: &minijinja::Value, name: &str) -> bool {
    let mut at = context.clone();
    for segment in name.split('.') {
        let Ok(next) = at.get_attr(segment) else {
            return false;
        };
        if next.is_undefined() {
            return false;
        }
        at = next;
    }
    true
}

/// A global is ambient rather than supplied, and only its own name is one —
/// `range.nope` is not a global because `range` is.
fn resolves_ambient(ambient: &BTreeSet<String>, name: &str) -> bool {
    ambient.contains(name)
}

/// The context's own keys, or `None` when it is not a map. A scalar or a
/// sequence has no names to list, and `try_iter` succeeds on a sequence — so
/// without the kind check its *elements* would be printed as if they were
/// variable names, and an uniterable value would read as "no variables", which
/// is a claim derived from a failure to look.
fn context_keys(context: &minijinja::Value) -> Option<BTreeSet<String>> {
    if context.kind() != minijinja::value::ValueKind::Map {
        return None;
    }
    Some(
        context
            .try_iter()
            .ok()?
            .map(|key| key.to_string())
            .collect(),
    )
}

/// Names in a sentence: backticked, comma-joined. Lives here because both the
/// library's render failures and the cli's reports build the same sentence out
/// of the same shape, and five hand-written copies of it once existed.
pub fn quoted<T: fmt::Display>(names: impl IntoIterator<Item = T>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `source` reads any of `names` at the top level, so a caller can
/// skip gathering context a template never looks at.
///
/// # Errors
///
/// Parse errors.
pub fn reads_any(source: &str, names: &[&str]) -> Result<bool, RenderError> {
    let mut env = Environment::new();
    env.add_template("probe", source)
        .map_err(|err| render_error(&err))?;
    let template = env
        .get_template("probe")
        .map_err(|err| render_error(&err))?;
    let read = template.undeclared_variables(false);
    Ok(names.iter().any(|name| read.contains(*name)))
}

fn render_error(err: &minijinja::Error) -> RenderError {
    RenderError {
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{render, TemplateSource};
    use minijinja::context;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Wrap {
        template: TemplateSource,
    }

    #[test]
    fn inline_source_round_trips_from_toml() {
        let wrap: Wrap = toml::from_str("template = \"hello {{ name }}\"\n").expect("inline");
        assert_eq!(
            wrap.template,
            TemplateSource::Inline(String::from("hello {{ name }}"))
        );
    }

    #[test]
    fn file_table_is_a_path() {
        let wrap: Wrap = toml::from_str("template = { file = \"notes.md\" }\n").expect("file");
        assert_eq!(
            wrap.template,
            TemplateSource::File(String::from("notes.md"))
        );
    }

    #[test]
    fn command_table_is_refused() {
        let err =
            toml::from_str::<Wrap>("template = { command = \"pandoc\" }\n").expect_err("command");
        assert!(err.to_string().contains("do not execute"), "{err}");
    }

    #[test]
    fn empty_file_and_unknown_keys_are_refused() {
        toml::from_str::<Wrap>("template = { file = \"\" }\n").expect_err("empty");
        toml::from_str::<Wrap>("template = { file = \" \" }\n").expect_err("blank");
        toml::from_str::<Wrap>("template = {}\n").expect_err("empty table");
        toml::from_str::<Wrap>("template = { file = \"a.md\", extra = \"x\" }\n")
            .expect_err("extra");
    }

    #[test]
    fn renders_a_defined_value() {
        let out = render("t", "v={{ version }}", context!(version => "1.2.3")).expect("render");
        assert_eq!(out, "v=1.2.3");
    }

    #[test]
    fn undefined_print_is_an_error() {
        let err = render("t", "{{ missing }}", context!()).expect_err("undef");
        assert!(
            err.to_string().contains("undefined") || err.to_string().contains("missing"),
            "{err}"
        );
    }

    /// minijinja says `undefined value (in tag-format:1)` and stops. The
    /// reporter who wrote `{{ name }}` found the right spelling by reading
    /// oakum's source; the failure now carries both halves of what they
    /// needed.
    #[test]
    fn an_undefined_variable_names_itself_and_the_scope() {
        let err = render(
            "tag-format",
            "{{ name }}",
            context!(version => "1.2.3", package => "demo"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("this template reads `name`, which the context does not supply"),
            "{err}"
        );
        assert!(err.contains("it receives `package`, `version`"), "{err}");
    }

    /// Every name the template asks for and the context lacks, not just the
    /// one that happened to render first, and a nested read is named the way
    /// it was written.
    #[test]
    fn the_scope_is_read_from_the_context_not_a_list() {
        let err = render(
            "title",
            "{{ nope }}{{ repo.slug }}",
            context!(version => "1"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("reads `nope`, `repo.slug`, which the context does not supply"),
            "{err}"
        );
        assert!(err.contains("it receives `version`"), "{err}");

        let empty = render("title", "{{ nope }}", context!())
            .expect_err("undefined")
            .to_string();
        assert!(empty.contains("it receives no variables"), "{empty}");
    }

    /// A template that reads a provided name and an unprovided one names only
    /// the second. Without the subtraction the message accuses `version` of
    /// being undefined on the same line that says the template receives it.
    #[test]
    fn a_name_the_context_supplies_is_not_named_as_missing() {
        let err = render("t", "{{ version }}{{ nope }}", context!(version => "1.2.3"))
            .expect_err("undefined")
            .to_string();
        assert!(
            err.contains("reads `nope`, which the context does not supply"),
            "{err}"
        );
        assert!(
            !err.contains("`nope`, `version`"),
            "a provided name is not missing: {err}"
        );
        assert!(err.contains("it receives `version`"), "{err}");
    }

    /// A nested read the context supplies is not missing. The analysis reports
    /// dotted paths while the context lists top-level names, so `repo.url` was
    /// accused of being unsupplied on the same line that listed `repo` as
    /// received — and `SectionContext.repo` is exactly that shape.
    #[test]
    fn a_nested_attribute_the_context_supplies_is_not_named_as_missing() {
        let err = render(
            "t",
            "{{ repo.url }}{{ nope }}",
            context!(repo => context!(url => "https://x/y"), version => "1"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("reads `nope`, which the context does not supply"),
            "{err}"
        );
        assert!(!err.contains("repo.url"), "a supplied nested read: {err}");
    }

    /// The mirror case, and the one the schema descriptions now invite by
    /// advertising `repo`: a nested read the context does *not* supply must
    /// name itself. Subtracting on the root alone silenced it into the generic
    /// sentence, which is what the finding asked to stop.
    #[test]
    fn a_nested_attribute_the_context_lacks_names_itself() {
        let err = render(
            "changelog",
            "{{ repo.nope }}",
            context!(repo => context!(url => "https://x/y"), version => "1"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("this template reads `repo.nope`, which the context does not supply"),
            "{err}"
        );
        assert!(
            !err.contains("a variable this template reads is not defined here"),
            "the generic sentence is the fallback, not the answer: {err}"
        );
    }

    /// A built-in is not a missing variable. `undeclared_variables` counts
    /// every name the template did not bind itself, so `range` arrives looking
    /// exactly like `nope` — and naming it would send a reader to define
    /// something minijinja already provides.
    #[test]
    fn a_template_global_is_not_reported_as_undefined() {
        let err = render(
            "t",
            "{% for i in range(3) %}{{ nope }}{% endfor %}",
            context!(version => "1"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("reads `nope`, which the context does not supply"),
            "{err}"
        );
        assert!(!err.contains("range"), "a global is not missing: {err}");
        // Subtracted, never listed: a global is ambient, not something this
        // surface hands the template.
        assert!(err.contains("it receives `version`"), "{err}");
    }

    /// A name the template reads only inside a branch that did not run is
    /// still a name the context does not supply — but it is not the cause of
    /// this render failing, and the sentence no longer says it is.
    #[test]
    fn the_message_states_a_set_not_a_cause() {
        let err = render(
            "t",
            "{% if false %}{{ ghost }}{% endif %}{{ nope }}",
            context!(version => "1"),
        )
        .expect_err("undefined")
        .to_string();
        assert!(
            err.contains("this template reads `ghost`, `nope`, which the context does not supply"),
            "{err}"
        );
        assert!(
            !err.contains("is not defined here"),
            "a set of names takes no singular verb: {err}"
        );
    }

    /// A context that is not a map has no names to list. `try_iter` succeeds on
    /// a sequence, so without the kind check its elements print as if they were
    /// variable names, and a value minijinja will not iterate reads as "no
    /// variables" — a claim derived from a failure to look, on the code path
    /// added to stop exactly that.
    #[test]
    fn a_context_that_is_not_a_map_is_not_reported_as_empty() {
        for shape in [
            minijinja::Value::from(vec!["alpha", "beta"]),
            minijinja::Value::from("a string"),
        ] {
            let err = render("t", "{{ nope }}", shape.clone())
                .expect_err("undefined")
                .to_string();
            assert!(
                err.contains("could not list the variables this template receives"),
                "{shape:?}: {err}"
            );
            assert!(
                !err.contains("it receives no variables"),
                "{shape:?}: {err}"
            );
            assert!(!err.contains("alpha"), "elements are not names: {err}");
        }
    }

    /// A failure that is not about an undefined name keeps its own message:
    /// the appended sentence is scoped to the one error kind it explains.
    #[test]
    fn other_render_failures_gain_no_scope_sentence() {
        let err = render(
            "t",
            "{{ version | nosuchfilter }}",
            context!(version => "1"),
        )
        .expect_err("unknown filter")
        .to_string();
        assert!(!err.contains("it receives"), "{err}");
    }

    #[test]
    fn undefined_in_if_is_false() {
        let out = render(
            "t",
            "{% if missing %}yes{% else %}no{% endif %}",
            context!(),
        )
        .expect("if");
        assert_eq!(out, "no");
    }

    #[test]
    fn include_without_a_loader_fails() {
        let err = render("t", "{% include 'other.md' %}", context!()).expect_err("include");
        assert!(!err.to_string().is_empty(), "{err}");
    }

    #[test]
    fn reads_any_sees_only_top_level_names() {
        assert!(super::reads_any("{{ repo.url }}", &["repo", "changes"]).unwrap());
        assert!(super::reads_any(
            "{% for c in changes %}{{ c.note }}{% endfor %}",
            &["repo", "changes"]
        )
        .unwrap());
        assert!(!super::reads_any("{{ version }} {{ notes[0] }}", &["repo", "changes"]).unwrap());
        assert!(super::reads_any("{{", &["repo"]).is_err());
    }
}
