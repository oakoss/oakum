//! User-owned templates: they render, they do not execute (ADR-0006).
//!
//! Two sources, and only two: an inline string, and `{ file = "path" }`.
//! `{ command = "..." }` is refused at parse. File paths are resolved by the
//! CLI at one containment chokepoint; this module only renders a body it is
//! given.

use std::collections::BTreeMap;
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
                if path.is_empty() {
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
    template.render(context).map_err(|err| render_error(&err))
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
}
