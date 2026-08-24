//! Load a [`oakum::template::TemplateSource`] through the repository Dir.
//!
//! File paths go through [`super::config::resolve_capability_path`] and nowhere
//! else, so a `{ file = ... }` value cannot escape the checkout (ADR-0006).

use std::io::Read;
use std::path::Path;

use cap_std::fs::Dir;
use oakum::template::TemplateSource;

use super::config::{open_read_only, resolve_capability_path};
use super::CliError;

pub(super) fn load_template_body(
    dir: &Dir,
    repo_path: &Path,
    source: &TemplateSource,
) -> Result<String, Box<dyn std::error::Error>> {
    match source {
        TemplateSource::Inline(body) => Ok(body.clone()),
        TemplateSource::File(relative) => {
            let resolved =
                resolve_capability_path(dir, repo_path, Path::new(relative)).map_err(|err| {
                    CliError::new(format!("failed to resolve template `{relative}`: {err}"))
                })?;
            let mut file = open_read_only(dir, &resolved).map_err(|err| {
                CliError::new(format!("failed to open template `{relative}`: {err}"))
            })?;
            let metadata = file.metadata().map_err(|err| {
                CliError::new(format!("failed to inspect template `{relative}`: {err}"))
            })?;
            if !metadata.is_file() {
                return Err(Box::new(CliError::new(format!(
                    "template `{relative}` is not a regular file"
                ))));
            }
            let mut body = String::new();
            file.read_to_string(&mut body).map_err(|err| {
                CliError::new(format!("failed to read template `{relative}`: {err}"))
            })?;
            Ok(body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::load_template_body;
    use cap_std::ambient_authority;
    use cap_std::fs::Dir;
    use oakum::template::TemplateSource;
    use std::fs;

    #[test]
    fn parent_escape_is_refused() {
        let root = std::env::temp_dir().join(format!("oakum-tpl-escape-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp");
        let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("dir");
        let err = load_template_body(
            &dir,
            &root,
            &TemplateSource::File(String::from("../secret.md")),
        )
        .expect_err("escape");
        assert!(err.to_string().contains("outside the repository"), "{err}");
        assert!(!err.to_string().contains("No such file"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_file_is_not_an_escape() {
        let root = std::env::temp_dir().join(format!("oakum-tpl-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp");
        let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("dir");
        let err = load_template_body(
            &dir,
            &root,
            &TemplateSource::File(String::from("missing.md")),
        )
        .expect_err("missing");
        assert!(
            err.to_string().contains("failed to resolve template"),
            "{err}"
        );
        assert!(!err.to_string().contains("outside the repository"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn file_inside_the_repo_is_read() {
        let root = std::env::temp_dir().join(format!("oakum-tpl-read-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp");
        fs::write(root.join("notes.md"), "hello {{ n }}\n").expect("notes");
        let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("dir");
        let body = load_template_body(&dir, &root, &TemplateSource::File(String::from("notes.md")))
            .expect("read");
        assert_eq!(body, "hello {{ n }}\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inline_body_is_returned() {
        let root = std::env::temp_dir().join(format!("oakum-tpl-inline-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp");
        let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("dir");
        let body = load_template_body(
            &dir,
            &root,
            &TemplateSource::Inline(String::from("hello {{ n }}")),
        )
        .expect("inline");
        assert_eq!(body, "hello {{ n }}");
        let _ = fs::remove_dir_all(&root);
    }
}
