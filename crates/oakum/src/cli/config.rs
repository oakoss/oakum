//! Shared `_config.toml` load for CLI commands.

use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_std::fs::{Dir, File};
use oakum::config::{self, OakumConfig};
use oakum::plan::PackageId;

use super::fs::{open_read_only, resolve_capability_path};
use super::repository::Repository;
use super::CliError;

const CONFIG_PATH: &str = ".changeset/_config.toml";

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

    pub(super) fn tool_version(&self) -> Option<&semver::Version> {
        self.inner.tool_version()
    }

    pub(super) fn versioning_for(&self, package: &str) -> oakum::plan::Versioning {
        self.inner.versioning_for(package)
    }

    pub(super) fn versioning(&self) -> oakum::plan::Versioning {
        self.inner.versioning()
    }

    pub(super) fn change_files(&self) -> bool {
        self.inner.change_files()
    }

    pub(super) fn conventional_commits(&self) -> bool {
        self.inner.conventional_commits()
    }

    pub(super) fn from_parsed(
        repo: &Repository,
        inner: OakumConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        contain_template_sources(repo, &inner)?;
        Ok(Self { inner })
    }

    pub(super) fn resolves_dependencies_at(
        &self,
        package: &str,
    ) -> Option<oakum::plan::ResolvesDependenciesAt> {
        self.inner.resolves_dependencies_at(package)
    }

    pub(super) fn extra_files_for(&self, package: &str) -> &[oakum::config::ExtraFile] {
        self.inner.extra_files_for(package)
    }

    pub(super) fn template(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.template()
    }

    pub(super) fn title(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.title()
    }

    pub(super) fn commit_message(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.commit_message()
    }

    pub(super) fn tag_format(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.tag_format()
    }

    pub(super) fn pr_status(&self) -> oakum::config::PrStatus {
        self.inner.pr_status()
    }

    #[must_use]
    pub(super) fn as_config(&self) -> &OakumConfig {
        &self.inner
    }

    pub(super) fn version_managed(&self, package: &oakum::plan::Package) -> bool {
        self.inner.version_managed(package)
    }

    pub(super) fn tag_managed(&self, package: &oakum::plan::Package) -> bool {
        self.inner.tag_managed(package)
    }

    /// Config may store `publish-command`; nothing executes it in v0 (ADR-0012).
    #[must_use]
    #[expect(
        dead_code,
        reason = "store-only until the publish slot is filled (ADR-0011 / ADR-0012)"
    )]
    pub(super) fn publish_command_for(&self, package: &str) -> Option<&str> {
        self.inner.publish_command_for(package)
    }

    pub(super) fn validate_selection_names<'a>(
        &self,
        known: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), CliError> {
        self.as_config()
            .validate_selection_names(known)
            .map_err(|err| CliError::new(err.to_string()))
    }

    pub(super) fn validate_workspace_selection(
        &self,
        workspace: &oakum::plan::Workspace,
    ) -> Result<(), CliError> {
        self.validate_selection_names(
            workspace
                .packages()
                .map(|package| package.id().name.as_str()),
        )
    }
}

/// Package ids that may own a bare tag / default write shape (ADR-0030).
pub(super) fn tag_managed_ids(
    workspace: &oakum::plan::Workspace,
    config: &LoadedConfig,
) -> BTreeSet<PackageId> {
    workspace
        .packages()
        .filter(|package| config.tag_managed(package))
        .map(|package| package.id().clone())
        .collect()
}

/// Missing `.changeset/_config.toml` → both intent mechanisms on.
pub(super) fn load_config(repo: &Repository) -> Result<LoadedConfig, Box<dyn std::error::Error>> {
    let Some(mut file) = open_config(repo.dir(), repo.path())? else {
        return Ok(LoadedConfig {
            inner: OakumConfig::defaults(),
        });
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|err| config_error(format!("failed to read `{CONFIG_PATH}`: {err}")))?;
    let inner = config::parse(&text).map_err(|err| {
        config_error(format!(
            "`{CONFIG_PATH}` is not a valid oakum config: {err}"
        ))
    })?;
    contain_template_sources(repo, &inner)?;
    Ok(LoadedConfig { inner })
}

pub(super) fn contain_template_sources(
    repo: &Repository,
    inner: &OakumConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    for (key, source) in inner.template_sources() {
        super::template::load_template_body(repo.dir(), repo.path(), source)
            .map_err(|err| config_error(format!("{key}: {err}")))?;
    }
    Ok(())
}

fn open_config(dir: &Dir, repo_path: &Path) -> Result<Option<File>, Box<dyn std::error::Error>> {
    open_config_before_open(dir, repo_path, || {})
}

/// Raw config text plus capability-resolved write targets for `upgrade`
/// (ADR-0023). `init` writes the same filenames through the repository `Dir`.
/// Resolution shares the read path's containment rules, so a symlinked
/// config cannot redirect the write outside the repository.
pub(super) struct ConfigSource {
    text: String,
    changeset_path: PathBuf,
    config_path: PathBuf,
}

impl ConfigSource {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// Resolved `.changeset` directory, relative to the repository capability.
    pub(super) fn changeset_path(&self) -> &Path {
        &self.changeset_path
    }

    /// Resolved `_config.toml`, relative to the repository capability.
    pub(super) fn config_path(&self) -> &Path {
        &self.config_path
    }
}

pub(super) fn read_config_source(
    repo: &Repository,
) -> Result<Option<ConfigSource>, Box<dyn std::error::Error>> {
    let dir = repo.dir();
    let repo_path = repo.path();
    let Some(mut file) = open_config(dir, repo_path)? else {
        return Ok(None);
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|err| config_error(format!("failed to read `{CONFIG_PATH}`: {err}")))?;
    let changeset_path = resolve_capability_path(dir, repo_path, Path::new(".changeset"))?;
    let config_path =
        resolve_capability_path(dir, repo_path, &changeset_path.join("_config.toml"))?;
    Ok(Some(ConfigSource {
        text,
        changeset_path,
        config_path,
    }))
}

/// Capability-resolved location for a file `upgrade` writes next to the
/// config. A missing file resolves to its literal path (it will be created);
/// an existing one goes through symlink containment like every read.
pub(super) fn resolve_sibling_write_target(
    repo: &Repository,
    changeset_path: &Path,
    file_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let literal = changeset_path.join(file_name);
    match repo.dir().symlink_metadata(&literal) {
        Ok(_) => resolve_capability_path(repo.dir(), repo.path(), &literal),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(literal),
        Err(err) => Err(config_error(format!(
            "failed to inspect `.changeset/{file_name}` within the repository: {err}"
        ))),
    }
}

fn open_config_before_open(
    dir: &Dir,
    repo_path: &Path,
    before_open: impl FnOnce(),
) -> Result<Option<File>, Box<dyn std::error::Error>> {
    let changeset_link = Path::new(".changeset");
    match dir.symlink_metadata(changeset_link) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(config_error(format!(
                "failed to inspect `.changeset` within the repository: {err}"
            )));
        }
    }
    let changeset_path = resolve_capability_path(dir, repo_path, changeset_link)?;
    let changeset = dir.open_dir(&changeset_path).map_err(|err| {
        config_error(format!(
            "failed to open `.changeset` within the repository: {err}"
        ))
    })?;
    match changeset.symlink_metadata("_config.toml") {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(config_error(format!(
                "failed to inspect `{CONFIG_PATH}` within the repository: {err}"
            )));
        }
    }
    let config_link = changeset_path.join("_config.toml");
    let config_path = resolve_capability_path(dir, repo_path, &config_link)?;
    let metadata = dir.metadata(&config_path).map_err(|err| {
        config_error(format!(
            "failed to inspect `{CONFIG_PATH}` within the repository: {err}"
        ))
    })?;
    if !metadata.is_file() {
        return Err(config_error(format!(
            "`{CONFIG_PATH}` does not resolve to a regular file"
        )));
    }
    before_open();
    let file = open_read_only(dir, &config_path).map_err(|err| {
        config_error(format!(
            "failed to open `{CONFIG_PATH}` within the repository: {err}"
        ))
    })?;
    let metadata = file
        .metadata()
        .map_err(|err| config_error(format!("failed to inspect `{CONFIG_PATH}`: {err}")))?;
    if !metadata.is_file() {
        return Err(config_error(format!(
            "`{CONFIG_PATH}` does not resolve to a regular file"
        )));
    }
    Ok(Some(file))
}

fn config_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(CliError::new(message))
}

/// ADR-0007: when config exists, refuse a `tool-version` that disagrees with this binary.
pub(super) fn enforce_tool_version(
    config: &LoadedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(configured) = config.tool_version() else {
        return Ok(());
    };
    let binary = env!("CARGO_PKG_VERSION")
        .parse::<semver::Version>()
        .expect("CARGO_PKG_VERSION is a semver version");
    if configured != &binary {
        return Err(Box::new(CliError::new(format!(
            "`tool-version` is `{configured}` but this binary is `{binary}`; run `oakum upgrade`"
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::process::{Command, Stdio};
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use cap_std::ambient_authority;

    use crate::test_fixture::Fixture;

    #[cfg(unix)]
    use super::super::repository;
    use super::{open_config, open_config_before_open, Dir};

    fn symlink(original: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        let original = original.as_ref();
        let link = link.as_ref();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(original, link)
        }
        #[cfg(windows)]
        {
            // `_config.toml` is the only file symlink this suite creates;
            // every other link is a directory (including dangling `.changeset`).
            if link.file_name().is_some_and(|name| name == "_config.toml") {
                std::os::windows::fs::symlink_file(original, link)
            } else {
                std::os::windows::fs::symlink_dir(original, link)
            }
        }
    }

    /// NTFS directory junction via `mklink /J` — no symlink privilege required.
    #[cfg(windows)]
    fn junction(target: &Path, link: &Path) -> std::io::Result<()> {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "mklink /J failed with {status}"
            )))
        }
    }

    fn fixture(label: &str) -> Fixture {
        Fixture::new("open-config", label)
    }

    fn open_root(root: &Path) -> Dir {
        Dir::open_ambient_dir(root, ambient_authority()).expect("repository capability")
    }

    fn canonical_root(root: &Path) -> PathBuf {
        fs::canonicalize(root).expect("canonical root")
    }

    fn read_config(root: &Path) -> String {
        let mut config = open_config(&open_root(root), &canonical_root(root))
            .expect("open config")
            .expect("config file");
        let mut text = String::new();
        config.read_to_string(&mut text).expect("read config");
        text
    }

    #[test]
    fn absent_config_in_existing_changeset_selects_defaults() {
        let root = fixture("absent");
        fs::create_dir(root.join(".changeset")).expect("changeset");

        let config =
            open_config(&open_root(&root), &canonical_root(&root)).expect("inspect config");

        assert!(config.is_none());
    }

    #[test]
    fn a_directory_config_is_not_a_regular_file() {
        let root = fixture("dir-config");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        fs::create_dir(root.join(".changeset/_config.toml")).expect("config dir");

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("a directory must not look like a missing config");
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[test]
    fn external_changeset_symlink_is_not_missing_config() {
        let root = fixture("external-ancestor");
        let external = fixture("external-target");
        fs::write(
            external.join("_config.toml"),
            "secret = \"must-not-be-read\"\n",
        )
        .expect("external config");
        symlink(&external, root.join(".changeset")).expect("changeset symlink");

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("external ancestor must fail");
        // Not the bare word "repository": every wrapper message in this module
        // carries it, so a generic open failure would satisfy that assertion.
        assert!(
            error
                .to_string()
                .contains("resolves outside the repository"),
            "{error}"
        );
    }

    /// The `..`-above-root branch is otherwise reached only from
    /// `cli::template`'s suite, so narrowing that one would strand it here.
    #[test]
    fn changeset_symlink_climbing_above_the_root_is_refused() {
        let root = fixture("climbing-ancestor");
        symlink("../..", root.join(".changeset")).expect("climbing symlink");

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("climbing ancestor must fail");
        assert!(
            error
                .to_string()
                .contains("resolves outside the repository"),
            "{error}"
        );
    }

    #[test]
    fn dangling_changeset_symlink_is_not_missing_config() {
        let root = fixture("dangling-ancestor");
        symlink("missing-changeset", root.join(".changeset")).expect("changeset symlink");

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("dangling ancestor must fail");
        assert!(error.to_string().contains("failed to"), "{error}");
    }

    #[test]
    fn internal_config_symlink_opens_its_target() {
        let root = fixture("internal-config");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        fs::write(
            root.join("oakum-config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"leaf-symlink-loaded\"\n",
        )
        .expect("config");
        symlink("../oakum-config.toml", root.join(".changeset/_config.toml"))
            .expect("config symlink");

        let text = read_config(&root);

        assert!(text.contains("leaf-symlink-loaded"), "{text}");
    }

    #[test]
    fn internal_changeset_symlink_opens_its_target() {
        let root = fixture("internal-changeset");
        let changeset = root.join("config/changeset");
        fs::create_dir_all(&changeset).expect("changeset");
        fs::write(
            changeset.join("_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"ancestor-symlink-loaded\"\n",
        )
        .expect("config");
        symlink("config/changeset", root.join(".changeset")).expect("changeset symlink");

        let text = read_config(&root);

        assert!(text.contains("ancestor-symlink-loaded"), "{text}");
    }

    #[test]
    fn absolute_internal_config_symlink_opens_its_target() {
        let root = fixture("absolute-internal-config");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let target = root.join("oakum-config.toml");
        fs::write(
            &target,
            "tool-version = \"0.0.0\"\ntitle = \"absolute-leaf-loaded\"\n",
        )
        .expect("config");
        symlink(&target, root.join(".changeset/_config.toml")).expect("config symlink");

        let text = read_config(&root);

        assert!(text.contains("absolute-leaf-loaded"), "{text}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn absolute_internal_symlink_accepts_filesystem_alias() {
        let root = fixture("absolute-alias");
        let canonical_root = fs::canonicalize(&root).expect("canonical root");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let target = root.join("oakum-config.toml");
        fs::write(
            &target,
            "tool-version = \"0.0.0\"\ntitle = \"absolute-alias-loaded\"\n",
        )
        .expect("config");
        symlink(&target, root.join(".changeset/_config.toml")).expect("config symlink");

        let mut config = open_config(&open_root(&root), &canonical_root)
            .expect("open config")
            .expect("config file");
        let mut text = String::new();
        config.read_to_string(&mut text).expect("read config");

        assert!(text.contains("absolute-alias-loaded"), "{text}");
    }

    #[test]
    fn absolute_internal_changeset_symlink_opens_its_target() {
        let root = fixture("absolute-internal-changeset");
        let changeset = root.join("config/changeset");
        fs::create_dir_all(&changeset).expect("changeset");
        fs::write(
            changeset.join("_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"absolute-ancestor-loaded\"\n",
        )
        .expect("config");
        symlink(&changeset, root.join(".changeset")).expect("changeset symlink");

        let text = read_config(&root);

        assert!(text.contains("absolute-ancestor-loaded"), "{text}");
    }

    #[test]
    fn changeset_symlink_to_repository_root_opens_its_target() {
        let root = fixture("changeset-to-root");
        fs::write(
            root.join("_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"repository-root-loaded\"\n",
        )
        .expect("config");
        symlink(&root, root.join(".changeset")).expect("changeset symlink");

        let text = read_config(&root);

        assert!(text.contains("repository-root-loaded"), "{text}");
    }

    #[test]
    fn nested_absolute_internal_symlink_opens_its_target() {
        let root = fixture("nested-absolute-internal");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let target = root.join("config-target");
        fs::create_dir(&target).expect("target directory");
        fs::write(
            target.join("oakum.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"nested-absolute-loaded\"\n",
        )
        .expect("config");
        symlink(&target, root.join("absolute-hop")).expect("absolute symlink");
        symlink(
            "../absolute-hop/oakum.toml",
            root.join(".changeset/_config.toml"),
        )
        .expect("config symlink");

        let text = read_config(&root);

        assert!(text.contains("nested-absolute-loaded"), "{text}");
    }

    #[test]
    fn parent_component_applies_after_symlink_resolution() {
        let root = fixture("symlink-parent");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        fs::create_dir_all(root.join("nested/deep")).expect("nested target");
        fs::write(
            root.join("nested/oakum.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"resolved-parent-loaded\"\n",
        )
        .expect("resolved config");
        fs::write(
            root.join("oakum.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"lexical-parent-wrong\"\n",
        )
        .expect("lexical config");
        symlink("nested/deep", root.join("jump")).expect("intermediate symlink");
        symlink(
            "../jump/../oakum.toml",
            root.join(".changeset/_config.toml"),
        )
        .expect("config symlink");

        let text = read_config(&root);

        assert!(text.contains("resolved-parent-loaded"), "{text}");
        assert!(!text.contains("lexical-parent-wrong"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn repository_capability_survives_root_replacement() {
        let root = fixture("stable-root");
        fs::create_dir(root.join(".git")).expect("git marker");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        fs::write(
            root.join(".changeset/_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"original-root\"\n",
        )
        .expect("original config");
        let repository = repository::discover_from(&root).expect("discover repository");
        // A sibling of the root, so it lands in the fixture's container and is
        // reclaimed with it.
        let moved = root.with_file_name("moved");
        fs::rename(&root, &moved).expect("rename repository");
        fs::create_dir(&root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");
        fs::create_dir(root.join(".changeset")).expect("replacement changeset");
        fs::write(
            root.join(".changeset/_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"replacement-root\"\n",
        )
        .expect("replacement config");

        let mut config = open_config(repository.dir(), repository.path())
            .expect("open config")
            .expect("config file");
        let mut text = String::new();
        config.read_to_string(&mut text).expect("read config");

        assert!(text.contains("original-root"), "{text}");
        assert!(!text.contains("replacement-root"), "{text}");
    }

    #[test]
    fn raced_external_config_symlink_is_rejected() {
        let root = fixture("raced-external-config");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let config_path = root.join(".changeset/_config.toml");
        fs::write(&config_path, "tool-version = \"0.0.0\"\n").expect("config");
        let external = fixture("raced-external-config-target");
        let external_config = external.join("config.toml");
        fs::write(
            &external_config,
            "secret = \"raced-external-must-not-be-read\"\n",
        )
        .expect("external config");
        let dir = open_root(&root);

        let result = open_config_before_open(&dir, &canonical_root(&root), || {
            fs::remove_file(&config_path).expect("remove regular config");
            symlink(&external_config, &config_path).expect("external config symlink");
        });

        let error = result.expect_err("external symlink must fail");
        // cap-std's sandbox refuses this one before oakum's own check sees it,
        // so its wording is what proves the refusal was for containment.
        assert!(
            error
                .to_string()
                .contains("a path led outside of the filesystem"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn raced_external_changeset_symlink_is_rejected() {
        let root = fixture("raced-external-changeset");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        fs::write(
            root.join(".changeset/_config.toml"),
            "tool-version = \"0.0.0\"\n",
        )
        .expect("config");
        let external = fixture("raced-external-changeset-target");
        fs::write(
            external.join("_config.toml"),
            "secret = \"raced-ancestor-must-not-be-read\"\n",
        )
        .expect("external config");
        let dir = open_root(&root);

        let result = open_config_before_open(&dir, &canonical_root(&root), || {
            fs::remove_file(root.join(".changeset/_config.toml")).expect("remove config");
            fs::remove_dir(root.join(".changeset")).expect("remove changeset");
            symlink(&external, root.join(".changeset")).expect("external changeset symlink");
        });

        let error = result.expect_err("external ancestor must fail");
        assert!(
            error
                .to_string()
                .contains("a path led outside of the filesystem"),
            "{error}"
        );
    }

    #[test]
    fn symlink_cycle_is_rejected() {
        let root = fixture("symlink-cycle");
        symlink("loop-a", root.join(".changeset")).expect("changeset symlink");
        symlink(".changeset", root.join("loop-a")).expect("loop symlink");

        let error =
            open_config(&open_root(&root), &canonical_root(&root)).expect_err("cycle must fail");

        assert!(
            error.to_string().contains("too many symbolic links"),
            "{error}"
        );
    }

    #[test]
    fn excessive_symlink_depth_is_rejected() {
        let root = fixture("symlink-depth");
        let target = root.join("target");
        fs::create_dir(&target).expect("target");
        fs::write(target.join("_config.toml"), "tool-version = \"0.0.0\"\n").expect("config");
        symlink("link-0", root.join(".changeset")).expect("changeset symlink");
        for index in 0..41 {
            let next = if index == 40 {
                String::from("target")
            } else {
                format!("link-{}", index + 1)
            };
            symlink(next, root.join(format!("link-{index}"))).expect("chain symlink");
        }

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("deep chain must fail");

        assert!(
            error.to_string().contains("too many symbolic links"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn raced_fifo_is_opened_nonblocking_and_rejected() {
        let root = fixture("raced-fifo-parent");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "cli::config::tests::raced_fifo_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("OAKUM_RACED_FIFO_ROOT", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("race helper");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll race helper") {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill blocked race helper");
                child.wait().expect("reap race helper");
                panic!("config open blocked after a regular file became a FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("race helper stderr")
            .read_to_string(&mut stderr)
            .expect("read race helper stderr");

        assert!(status.success(), "race helper failed: {stderr}");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "run by raced_fifo_is_opened_nonblocking_and_rejected"]
    fn raced_fifo_helper() {
        let root = PathBuf::from(std::env::var_os("OAKUM_RACED_FIFO_ROOT").expect("fixture root"));
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let config_path = root.join(".changeset/_config.toml");
        fs::write(&config_path, "tool-version = \"0.0.0\"\n").expect("config");
        let dir = open_root(&root);

        let result = open_config_before_open(&dir, &canonical_root(&root), || {
            fs::remove_file(&config_path).expect("remove regular config");
            let status = Command::new("mkfifo")
                .arg(&config_path)
                .status()
                .expect("mkfifo");
            assert!(status.success(), "mkfifo: {status}");
        });

        let error = result.expect_err("FIFO must fail");
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[cfg(windows)]
    fn flipped_drive_case(path: &Path) -> PathBuf {
        let mut text = path.to_string_lossy().into_owned();
        let pos = text
            .char_indices()
            .find_map(|(idx, ch)| ch.is_ascii_alphabetic().then_some(idx))
            .unwrap_or_else(|| panic!("no drive letter in {}", path.display()));
        assert_eq!(
            text.as_bytes().get(pos + 1),
            Some(&b':'),
            "no drive colon in {}",
            path.display()
        );
        let ch = text.as_bytes()[pos];
        let flipped = if ch.is_ascii_uppercase() {
            ch.to_ascii_lowercase()
        } else {
            ch.to_ascii_uppercase()
        };
        text.replace_range(pos..=pos, std::str::from_utf8(&[flipped]).expect("ascii"));
        PathBuf::from(text)
    }

    /// Flip ASCII letter case after the drive (`C:` / `\\?\C:`). NTFS treats
    /// `Users` and `users` as the same path; containment must too (okm-3l8.3).
    #[cfg(windows)]
    fn flipped_path_body_case(path: &Path) -> PathBuf {
        let chars: Vec<char> = path.to_string_lossy().chars().collect();
        let mut out = String::with_capacity(chars.len());
        let mut after_drive = false;
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if !after_drive
                && ch == ':'
                && chars
                    .get(i + 1)
                    .is_some_and(|next| *next == '\\' || *next == '/')
            {
                out.push(ch);
                after_drive = true;
            } else if after_drive && ch.is_ascii_alphabetic() {
                out.push(if ch.is_ascii_uppercase() {
                    ch.to_ascii_lowercase()
                } else {
                    ch.to_ascii_uppercase()
                });
            } else {
                out.push(ch);
            }
            i += 1;
        }
        assert!(after_drive, "expected a drive path, got {}", path.display());
        PathBuf::from(out)
    }

    #[cfg(windows)]
    fn verbatim_prefix(path: &Path) -> PathBuf {
        let text = path.to_string_lossy();
        if text.starts_with(r"\\?\") {
            path.to_path_buf()
        } else {
            PathBuf::from(format!(r"\\?\{text}"))
        }
    }

    #[cfg(windows)]
    fn loopback_admin_unc(path: &Path) -> PathBuf {
        let normalized = super::super::fs::normalized_windows_path(path);
        assert!(
            normalized.len() >= 3 && normalized.as_bytes()[1] == b':',
            "expected a drive path, got {normalized}"
        );
        let drive = normalized.chars().next().expect("drive");
        let tail = &normalized[2..];
        PathBuf::from(format!(r"\\localhost\{drive}${tail}"))
    }

    #[cfg(windows)]
    fn read_config_at(root: &Path, repo_path: &Path) -> String {
        let mut config = open_config(&open_root(root), repo_path)
            .expect("open config")
            .expect("config file");
        let mut text = String::new();
        config.read_to_string(&mut text).expect("read config");
        text
    }

    #[cfg(windows)]
    fn write_absolute_internal_config(root: &Path, title: &str) {
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let target = root.join("oakum-config.toml");
        fs::write(
            &target,
            format!("tool-version = \"0.0.0\"\ntitle = \"{title}\"\n"),
        )
        .expect("config");
        symlink(&target, root.join(".changeset/_config.toml")).expect("config symlink");
    }

    #[cfg(windows)]
    #[test]
    fn drive_letter_case_does_not_escape_an_internal_config() {
        let root = fixture("drive-case");
        write_absolute_internal_config(&root, "drive-case-loaded");
        let canonical = canonical_root(&root);
        let flipped = flipped_drive_case(&canonical);
        assert_ne!(
            flipped.to_string_lossy(),
            canonical.to_string_lossy(),
            "need a case difference to exercise ignore-ascii"
        );

        let text = read_config_at(&root, &flipped);
        assert!(text.contains("drive-case-loaded"), "{text}");
    }

    /// NTFS folds path components (`Users` ≡ `users`). The discovery-time
    /// `repo_path` may not match the casing `canonicalize` returns for a
    /// symlink target; containment must still accept the pair (okm-3l8.3).
    #[cfg(windows)]
    #[test]
    fn path_component_case_does_not_escape_an_internal_config() {
        let root = fixture("path-case");
        write_absolute_internal_config(&root, "path-case-loaded");
        let canonical = canonical_root(&root);
        let flipped = flipped_path_body_case(&canonical);
        assert_ne!(
            flipped.to_string_lossy(),
            canonical.to_string_lossy(),
            "need a path-body case difference to exercise NTFS ignore-ascii"
        );

        let text = read_config_at(&root, &flipped);
        assert!(text.contains("path-case-loaded"), "{text}");
    }

    /// Directory junction (mklink /J) is the common Windows stand-in for an
    /// in-repo directory symlink and does not need SeCreateSymbolicLinkPrivilege
    /// (okm-3l8.4).
    #[cfg(windows)]
    #[test]
    fn internal_changeset_junction_opens_its_target() {
        let root = fixture("changeset-junction");
        let changeset = root.join("config").join("changeset");
        fs::create_dir_all(&changeset).expect("changeset");
        fs::write(
            changeset.join("_config.toml"),
            "tool-version = \"0.0.0\"\ntitle = \"junction-loaded\"\n",
        )
        .expect("config");
        junction(&changeset, &root.join(".changeset")).expect("changeset junction");

        let text = read_config(&root);
        assert!(text.contains("junction-loaded"), "{text}");

        let resolved = super::super::fs::resolve_capability_path(
            &open_root(&root),
            &canonical_root(&root),
            Path::new(".changeset/_config.toml"),
        )
        .expect("resolve through junction");
        assert_eq!(
            resolved,
            Path::new("config/changeset/_config.toml"),
            "resolve must follow the junction to the real target, got {resolved:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn absolute_internal_file_symlink_opens_its_target() {
        let root = fixture("windows-abs-file-symlink");
        write_absolute_internal_config(&root, "windows-abs-file-loaded");

        let text = read_config(&root);
        assert!(text.contains("windows-abs-file-loaded"), "{text}");

        let resolved = super::super::fs::resolve_capability_path(
            &open_root(&root),
            &canonical_root(&root),
            Path::new(".changeset/_config.toml"),
        )
        .expect("resolve through absolute file symlink");
        assert_eq!(
            resolved,
            Path::new("oakum-config.toml"),
            "resolve must follow the absolute file symlink, got {resolved:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_does_not_escape_an_internal_config() {
        let root = fixture("verbatim");
        write_absolute_internal_config(&root, "verbatim-loaded");

        let text = read_config_at(&root, &verbatim_prefix(root.as_ref()));
        assert!(text.contains("verbatim-loaded"), "{text}");
    }

    #[cfg(windows)]
    #[test]
    fn absolute_internal_config_survives_a_unc_repo_prefix() {
        let root = fixture("unc-absolute-internal");
        fs::create_dir(root.join(".changeset")).expect("changeset");
        let target = root.join("oakum-config.toml");
        fs::write(
            &target,
            "tool-version = \"0.0.0\"\ntitle = \"unc-absolute-loaded\"\n",
        )
        .expect("config");
        symlink(&target, root.join(".changeset/_config.toml")).expect("config symlink");

        let text = read_config_at(&root, &loopback_admin_unc(&canonical_root(&root)));
        assert!(text.contains("unc-absolute-loaded"), "{text}");
    }

    #[cfg(windows)]
    #[test]
    fn external_unc_target_is_rejected() {
        let root = fixture("unc-external");
        let external = fixture("unc-external-target");
        fs::write(
            external.join("_config.toml"),
            "secret = \"unc-must-not-be-read\"\n",
        )
        .expect("external config");
        symlink(loopback_admin_unc(&external), root.join(".changeset")).expect("changeset symlink");

        let error = open_config(
            &open_root(&root),
            &loopback_admin_unc(&canonical_root(&root)),
        )
        .expect_err("external UNC ancestor must fail");
        assert!(
            error
                .to_string()
                .contains("resolves outside the repository"),
            "{error}"
        );
    }
}
