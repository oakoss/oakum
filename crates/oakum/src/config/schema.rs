//! The JSON Schema `_schema.json` carries, written beside the parser rather
//! than derived from it.

use std::path::{Component, Path};

use serde_json::{json, Value};

use super::VERSIONING_DESCRIPTION;

fn extra_files_schema() -> Value {
    json!({
        "type": "array",
        "description": "Declared version writes outside the package manifest (ADR-0033).",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["path", "format", "key"],
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Leading `/` is repository-root relative; otherwise relative to the package manifest directory.",
                },
                "format": {
                    "type": "string",
                    "enum": ["json"],
                    "description": "Document format. v1 ships json only.",
                },
                "key": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Dotted key path. `{field=value}` selects a unique array element (ADR-0033).",
                },
            },
        },
    })
}

fn template_source_schema(description: &str) -> Value {
    json!({
        "description": description,
        "oneOf": [
            { "type": "string" },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["file"],
                "properties": {
                    "file": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": r".*\S.*",
                    },
                },
            },
        ],
    })
}

/// JSON Schema for `.changeset/_config.toml`. `init` / `upgrade` will write it
/// next to the file; taplo reads it via `#:schema ./_schema.json`.
#[must_use]
pub fn schema() -> Value {
    let versioning = json!({
        "type": "string",
        "enum": ["zero-major", "semver"],
        "description": VERSIONING_DESCRIPTION,
    });
    let mut versioning_root = versioning.clone();
    versioning_root["default"] = json!("zero-major");
    json!({
        "$schema": "https://json-schema.org/draft-07/schema#",
        "title": "oakum _config.toml",
        "type": "object",
        "additionalProperties": false,
        "if": {
            "properties": {
                "change-files": { "const": false },
                "conventional-commits": { "const": false },
            },
            "required": ["change-files", "conventional-commits"],
        },
        "then": false,
        "properties": {
            "tool-version": {
                "description": "Deprecated: oakum no longer reads this key, and `oakum upgrade` removes it. The install pin is the version (ADR-0038).",
            },
            "change-files": {
                "type": "boolean",
                "default": true,
                "description": "When true, the plan reads `.changeset/*.md`.",
            },
            "conventional-commits": {
                "type": "boolean",
                "default": true,
                "description": "When true, commits can feed `generate`, and the plan if change-files is off (ADR-0029).",
            },
            "versioning": versioning_root,
            "pr-status": {
                "type": "string",
                "enum": ["comment", "summary", "both", "none"],
                "default": "both",
                "description": "Pull-request presentation. `none` silences comment and summary; the exit-code gate is not configurable (ADR-0015).",
            },
            "tag-format": template_source_schema(
                "Tag oakum writes. Receives `package` and `version`. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Existing tags are derived, not configured (ADR-0004).",
            ),
            "commit-message": template_source_schema(
                "Commit message for the version commit. Receives the `oakum status --json` document: `awaiting_tag`, `changed`, `coverage`, `hand_set`, `manages_nothing`, `named_unmanaged`, `packages`, `schema_version`, `selection_empty`, `target`, `uncovered`, `unmanaged`, `unresolved_scopes`. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).",
            ),
            "title": template_source_schema(
                "Title for the version pull request. Defaults to the version commit's built-in message; a configured `commit-message` does not change it. Where a repository takes its squash subject from the title, that default keeps the subject conventional. Receives the `oakum status --json` document: `awaiting_tag`, `changed`, `coverage`, `hand_set`, `manages_nothing`, `named_unmanaged`, `packages`, `schema_version`, `selection_empty`, `target`, `uncovered`, `unmanaged`, `unresolved_scopes`. A string is inline; `{ file = \"path\" }` loads a repository-relative file. One template per surface, with conditionals in the body (ADR-0015).",
            ),
            "template": template_source_schema(
                "Changelog template, rendered once per package section. Receives `bump`, `changes`, `date`, `ecosystem`, `notes`, `package`, `repo`, `source`, `target`, `tool_version`, `trigger`, `version`. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).",
            ),
            "private-packages": {
                "type": "object",
                "additionalProperties": false,
                "default": { "version": false, "tag": false },
                "description": "Opt-in to version and/or tag packages that are not registry-publishable. Axes are independent (ADR-0027).",
                "properties": {
                    "version": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, unpublishable packages still receive version bumps and changelogs.",
                    },
                    "tag": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, unpublishable packages still receive git tags.",
                    },
                },
            },
            "include": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "pattern": r".*\S.*" },
                "default": [],
                "description": "Exact package names to manage. Empty means all packages; then exclude removes (ADR-0027).",
            },
            "exclude": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "pattern": r".*\S.*" },
                "default": [],
                "description": "Exact package names to leave alone after include filtering (ADR-0027).",
            },
            "packages": {
                "type": "object",
                "description": "Per-package overrides, keyed by the name the manifest declares.",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "versioning": versioning,
                        "resolves-dependencies-at": {
                            "type": "string",
                            "enum": ["build"],
                            "description": "Declare that this library bundles dependencies into the published artifact. Binaries are derived; `install` is not configurable (ADR-0009).",
                        },
                        "extra-files": extra_files_schema(),
                        "publish-command": {
                            "type": "string",
                            "minLength": 1,
                            "pattern": r".*\S.*",
                            "description": "Stored publish command for a post-v0 registry slot. Never executed in v0 (ADR-0011 / ADR-0012).",
                        },
                    },
                },
            },
        },
    })
}

/// Pretty JSON Schema plus a trailing newline, the bytes `init` writes.
///
/// # Panics
///
/// Never: `schema()` is a `serde_json::Value`, which always serializes.
#[must_use]
pub fn schema_json() -> String {
    crate::prettier_json::to_prettier_string(&schema())
}

/// Whether `text` points taplo at the `_schema.json` committed beside it: a
/// `#:schema` directive among the comments that open the file, naming that
/// sibling. A URL, an absolute path, or a path into `node_modules` names a
/// schema oakum does not keep.
#[must_use]
pub fn names_committed_schema(text: &str) -> bool {
    let directive = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .take_while(|line| line.is_empty() || line.starts_with('#'))
        .find_map(|line| line.strip_prefix("#:schema"))
        .map(str::trim);
    directive.is_some_and(|path| {
        // Lexically, from `.changeset/`: the file may not exist to resolve.
        let mut resolved: Vec<&std::ffi::OsStr> = vec![".changeset".as_ref()];
        for component in Path::new(path).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if resolved.pop().is_none() {
                        return false;
                    }
                }
                Component::Normal(name) => resolved.push(name),
                Component::RootDir | Component::Prefix(_) => return false,
            }
        }
        resolved == [".changeset", "_schema.json"]
    })
}
