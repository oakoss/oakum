//! Shared `_config.toml` load for CLI commands (tool-version + intent switches).

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::CliError;

/// Defaults when keys are omitted: both intent mechanisms on (ADR-0019 / ADR-0029).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadedConfig {
    tool_version: Option<String>,
    change_files: bool,
    conventional_commits: bool,
}

/// What feeds the plan (ADR-0029 single-artifact table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanIntentSource {
    ChangeFiles,
    /// Commit-derived intent; never writes a bump file.
    CommitsOnly,
}

impl LoadedConfig {
    pub(super) fn defaults_both_on() -> Self {
        Self {
            tool_version: None,
            change_files: true,
            conventional_commits: true,
        }
    }

    /// ADR-0029: `generate` needs both mechanisms enabled.
    pub(super) fn generate_allowed(&self) -> bool {
        self.change_files && self.conventional_commits
    }

    /// ADR-0029 plan input. Both mechanisms off is invalid.
    ///
    /// # Errors
    ///
    /// When both `change-files` and `conventional-commits` are disabled.
    pub(super) fn plan_intent_source(&self) -> Result<PlanIntentSource, CliError> {
        match (self.change_files, self.conventional_commits) {
            (true, _) => Ok(PlanIntentSource::ChangeFiles),
            (false, true) => Ok(PlanIntentSource::CommitsOnly),
            (false, false) => Err(CliError::new(
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(rename = "tool-version")]
    tool_version: String,
    #[serde(default = "default_true", rename = "change-files")]
    change_files: bool,
    #[serde(default = "default_true", rename = "conventional-commits")]
    conventional_commits: bool,
    /// Accepted so configs written by `init --versioning` (ADR-0022) still load; unused here.
    #[serde(default, rename = "versioning")]
    _versioning: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Missing `.changeset/_config.toml` → both intent mechanisms on.
pub(super) fn load_config(repo: &Path) -> Result<LoadedConfig, Box<dyn std::error::Error>> {
    let path = repo.join(".changeset/_config.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedConfig::defaults_both_on());
        }
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `{}`: {err}",
                path.display()
            ))));
        }
    };
    let parsed: ConfigFile = toml::from_str(&text).map_err(|err| {
        Box::new(CliError::new(format!(
            "`{}` is not a valid oakum config: {err}",
            path.display()
        ))) as Box<dyn std::error::Error>
    })?;
    Ok(LoadedConfig {
        tool_version: Some(parsed.tool_version),
        change_files: parsed.change_files,
        conventional_commits: parsed.conventional_commits,
    })
}

/// ADR-0007: when config exists, refuse a `tool-version` that disagrees with this binary.
pub(super) fn enforce_tool_version(
    config: &LoadedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(configured) = config.tool_version.as_deref() else {
        return Ok(());
    };
    let binary = env!("CARGO_PKG_VERSION");
    if configured != binary {
        return Err(Box::new(CliError::new(format!(
            "`tool-version` is `{configured}` but this binary is `{binary}`; run `oakum upgrade`"
        ))));
    }
    Ok(())
}
