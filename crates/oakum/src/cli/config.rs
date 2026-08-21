//! Shared `_config.toml` load for CLI commands.

use std::fs;
use std::path::Path;

use oakum::config::{self, OakumConfig};

use super::CliError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadedConfig {
    inner: OakumConfig,
}

/// What feeds the plan (ADR-0029 single-artifact table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanIntentSource {
    ChangeFiles,
    /// Commit-derived intent; never writes a bump file.
    CommitsOnly,
}

impl LoadedConfig {
    /// ADR-0029: `generate` needs both mechanisms enabled.
    pub(super) fn generate_allowed(&self) -> bool {
        self.inner.change_files() && self.inner.conventional_commits()
    }

    /// ADR-0029 plan input.
    ///
    /// # Errors
    ///
    /// When both `change-files` and `conventional-commits` are disabled.
    pub(super) fn plan_intent_source(&self) -> Result<PlanIntentSource, CliError> {
        match (self.inner.change_files(), self.inner.conventional_commits()) {
            (true, _) => Ok(PlanIntentSource::ChangeFiles),
            (false, true) => Ok(PlanIntentSource::CommitsOnly),
            (false, false) => Err(CliError::new(
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
            )),
        }
    }

    pub(super) fn tool_version(&self) -> Option<&str> {
        self.inner.tool_version()
    }
}

/// Missing `.changeset/_config.toml` → both intent mechanisms on.
pub(super) fn load_config(repo: &Path) -> Result<LoadedConfig, Box<dyn std::error::Error>> {
    let path = repo.join(".changeset/_config.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedConfig {
                inner: OakumConfig::defaults(),
            });
        }
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `{}`: {err}",
                path.display()
            ))));
        }
    };
    let inner = config::parse(&text).map_err(|err| {
        Box::new(CliError::new(format!(
            "`{}` is not a valid oakum config: {err}",
            path.display()
        ))) as Box<dyn std::error::Error>
    })?;
    Ok(LoadedConfig { inner })
}

/// ADR-0007: when config exists, refuse a `tool-version` that disagrees with this binary.
pub(super) fn enforce_tool_version(
    config: &LoadedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(configured) = config.tool_version() else {
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
