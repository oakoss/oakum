//! Shared `_config.toml` load for CLI commands.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::fs::{Dir, File, OpenOptions};
use oakum::config::{self, OakumConfig};

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

    pub(super) fn template(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.template()
    }

    pub(super) fn title(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.title()
    }

    pub(super) fn commit_message(&self) -> Option<&oakum::template::TemplateSource> {
        self.inner.commit_message()
    }

    pub(super) fn pr_status(&self) -> oakum::config::PrStatus {
        self.inner.pr_status()
    }
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

/// Replace `target` via a sibling temp file so rename stays on one filesystem
/// (no EXDEV across mounts). Staging uses `create_new` so a pre-existing
/// path cannot redirect the write. On collision, pick another name rather
/// than removing the entry; sweeping orphans would reintroduce that race.
pub(super) fn write_file_via_rename(
    dir: &Dir,
    target: &Path,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::new("write target has no file name"))?;
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut attempt: u32 = 0;
    let (tmp, mut staged) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let candidate = parent.join(format!(
            ".{file_name}.oakum-write.{}.{nanos}.{attempt}",
            std::process::id()
        ));
        match dir.open_with(&candidate, OpenOptions::new().create_new(true).write(true)) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(err) => {
                return Err(Box::new(CliError::new(format!(
                    "failed to stage `{file_name}`: {err}"
                ))));
            }
        }
    };
    staged.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to stage `{file_name}`: {err}"))
    })?;
    drop(staged);
    dir.rename(&tmp, dir, target).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to replace `{file_name}`: {err}"))
    })?;
    Ok(())
}

/// Unlike [`write_file_via_rename`], this never replaces a file that appears
/// between the check and the write.
pub(super) fn write_file_exclusive(
    dir: &Dir,
    target: &Path,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::new("write target has no file name"))?;
    let mut file = dir
        .open_with(target, OpenOptions::new().create_new(true).write(true))
        .map_err(|err| CliError::new(format!("failed to create `{file_name}`: {err}")))?;
    file.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(target);
        CliError::new(format!("failed to write `{file_name}`: {err}"))
    })?;
    Ok(())
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

pub(super) fn resolve_capability_path(
    dir: &Dir,
    repo_path: &Path,
    path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut pending = relative_components(path)?;
    let mut resolved = PathBuf::new();
    let mut followed = 0;
    while let Some(component) = pending.pop_front() {
        match component {
            PendingComponent::Parent => {
                if !resolved.pop() {
                    return Err(outside_repository(path));
                }
            }
            PendingComponent::Normal(component) => {
                let candidate = resolved.join(&component);
                let metadata = dir.symlink_metadata(&candidate).map_err(|err| {
                    config_error(format!(
                        "failed to resolve `{}` within the repository: {err}",
                        path.display()
                    ))
                })?;
                if !metadata.file_type().is_symlink() {
                    resolved.push(component);
                    continue;
                }
                followed += 1;
                if followed > 40 {
                    return Err(config_error(format!(
                        "`{}` contains too many symbolic links",
                        path.display()
                    )));
                }
                let target = dir.read_link_contents(&candidate).map_err(|err| {
                    config_error(format!(
                        "failed to resolve `{}` within the repository: {err}",
                        path.display()
                    ))
                })?;
                let target = if target.is_absolute() {
                    resolved.clear();
                    contained_absolute_target(repo_path, &target)
                        .ok_or_else(|| outside_repository(path))?
                } else {
                    target
                };
                let mut target_components = relative_components(&target)?;
                while let Some(target_component) = target_components.pop_back() {
                    pending.push_front(target_component);
                }
            }
        }
    }
    if resolved.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(resolved)
    }
}

enum PendingComponent {
    Parent,
    Normal(OsString),
}

fn relative_components(
    path: &Path,
) -> Result<VecDeque<PendingComponent>, Box<dyn std::error::Error>> {
    let mut components = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => {
                components.push_back(PendingComponent::Normal(component.to_owned()));
            }
            Component::CurDir => {}
            Component::ParentDir => components.push_back(PendingComponent::Parent),
            Component::RootDir | Component::Prefix(_) => return Err(outside_repository(path)),
        }
    }
    Ok(components)
}

#[cfg(not(windows))]
fn contained_absolute_target(repo_path: &Path, target: &Path) -> Option<PathBuf> {
    fs::canonicalize(target)
        .ok()?
        .strip_prefix(repo_path)
        .ok()
        .map(Path::to_path_buf)
}

#[cfg(windows)]
fn contained_absolute_target(repo_path: &Path, target: &Path) -> Option<PathBuf> {
    let repo = normalized_windows_path(repo_path);
    let target = normalized_windows_path(&fs::canonicalize(target).ok()?);
    contained_windows_path(&repo, &target)
}

#[cfg(any(windows, test))]
fn contained_windows_path(repo: &str, target: &str) -> Option<PathBuf> {
    let prefix = target.get(..repo.len())?;
    if !prefix.eq_ignore_ascii_case(repo) {
        return None;
    }
    let remainder = target.get(repo.len()..)?;
    let repo_ends_with_separator = repo.ends_with('\\') || repo.ends_with('/');
    if !remainder.is_empty()
        && !repo_ends_with_separator
        && !remainder.starts_with('\\')
        && !remainder.starts_with('/')
    {
        return None;
    }
    Some(PathBuf::from(remainder.trim_start_matches(|character| {
        character == '\\' || character == '/'
    })))
}

#[cfg(windows)]
fn normalized_windows_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
    }
}

fn outside_repository(path: &Path) -> Box<dyn std::error::Error> {
    config_error(format!(
        "`{}` resolves outside the repository",
        path.display()
    ))
}

pub(super) fn open_read_only(dir: &Dir, path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    dir.open_with(path, &options)
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

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use cap_std::ambient_authority;

    use super::super::repository;
    use super::{contained_windows_path, open_config, open_config_before_open, Dir};

    fn fixture(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("oakum-open-config-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("fixture root");
        root
    }

    #[test]
    fn windows_drive_root_contains_files() {
        assert_eq!(
            contained_windows_path(r"C:\", r"c:\config.toml"),
            Some(PathBuf::from("config.toml"))
        );
        assert_eq!(
            contained_windows_path(r"C:\repo", r"c:\repository\config.toml"),
            None
        );
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
        fs::remove_dir_all(root).expect("remove fixture");
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
        fs::remove_dir_all(&root).expect("remove fixture");
        fs::remove_dir_all(external).expect("remove external fixture");
        assert!(error.to_string().contains("repository"), "{error}");
    }

    #[test]
    fn dangling_changeset_symlink_is_not_missing_config() {
        let root = fixture("dangling-ancestor");
        symlink("missing-changeset", root.join(".changeset")).expect("changeset symlink");

        let error = open_config(&open_root(&root), &canonical_root(&root))
            .expect_err("dangling ancestor must fail");
        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
        assert!(text.contains("resolved-parent-loaded"), "{text}");
        assert!(!text.contains("lexical-parent-wrong"), "{text}");
    }

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
        let moved = root.with_file_name(format!(
            "oakum-open-config-stable-root-moved-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&moved);
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

        fs::remove_dir_all(root).expect("remove replacement");
        fs::remove_dir_all(moved).expect("remove original");
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
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(external).expect("remove external fixture");
        assert!(error.to_string().contains("repository"), "{error}");
    }

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
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(external).expect("remove external fixture");
        assert!(error.to_string().contains("repository"), "{error}");
    }

    #[test]
    fn symlink_cycle_is_rejected() {
        let root = fixture("symlink-cycle");
        symlink("loop-a", root.join(".changeset")).expect("changeset symlink");
        symlink(".changeset", root.join("loop-a")).expect("loop symlink");

        let error =
            open_config(&open_root(&root), &canonical_root(&root)).expect_err("cycle must fail");

        fs::remove_dir_all(root).expect("remove fixture");
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

        fs::remove_dir_all(root).expect("remove fixture");
        assert!(
            error.to_string().contains("too many symbolic links"),
            "{error}"
        );
    }

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
        fs::remove_dir_all(root).expect("remove fixture");

        assert!(status.success(), "race helper failed: {stderr}");
    }

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
}
