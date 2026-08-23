//! Read the repository for foreign release-tool markers (`okm-0s5`).

use std::io::{self, Read};

use cap_std::fs::{Dir, OpenOptions};
use oakum::detect::{
    detect, is_release_config_name, is_releaserc_name, DetectInput, DetectReport, Detection,
};

use super::repository;
use super::CliError;

/// `.releaserc` / `.releaserc.*` and `release.config.{js,cjs,mjs,ts}` come from the root listing.
const PATH_MARKERS: &[&str] = &[
    "knope.toml",
    ".changeset/config.json",
    ".bumpy/_config.json",
    "release-please-config.json",
    ".release-please-manifest.json",
    "release-plz.toml",
    ".release-plz.toml",
    "Cargo.toml",
    "package.json",
    "nx.json",
];

pub(super) fn run() -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let report = scan(repo.dir())?;
    print_detections(&report.detections);
    if !report.errors.is_empty() {
        let joined = report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(CliError::unverified(format!("unverified: {joined}")));
    }
    Ok(())
}

/// A parse failure is unverified, not "no tool".
pub(super) fn scan(dir: &Dir) -> Result<DetectReport, CliError> {
    let mut relative = Vec::new();
    for path in PATH_MARKERS {
        if file_exists(dir, path)? {
            relative.push((*path).to_string());
        }
    }
    for name in list_files(dir, ".").map_err(|err| {
        CliError::unverified(format!(
            "unverified: failed to list the repository root: {err}"
        ))
    })? {
        if is_releaserc_name(&name) || is_release_config_name(&name) {
            relative.push(name);
        }
    }

    let changeset_names = match list_files(dir, ".changeset") {
        Ok(names) => {
            for name in &names {
                relative.push(format!(".changeset/{name}"));
            }
            Some(names)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(CliError::unverified(format!(
                "unverified: failed to read `.changeset/`: {err}"
            )));
        }
    };

    let cargo_toml = read_optional(dir, "Cargo.toml")?;
    let package_json = read_optional(dir, "package.json")?;
    let nx_json = read_optional(dir, "nx.json")?;

    let relative_refs: Vec<&str> = relative.iter().map(String::as_str).collect();
    let changeset_refs: Option<Vec<&str>> = changeset_names
        .as_ref()
        .map(|names| names.iter().map(String::as_str).collect());

    Ok(detect(&DetectInput {
        relative_paths: &relative_refs,
        changeset_names: changeset_refs.as_deref(),
        cargo_toml: cargo_toml.as_deref(),
        package_json: package_json.as_deref(),
        nx_json: nx_json.as_deref(),
    }))
}

fn print_detections(found: &[Detection]) {
    for hit in found {
        println!("{}\t{}", hit.tool().name(), hit.evidence());
    }
    if !found.is_empty() {
        eprintln!("run oakum migrate");
    }
}

fn file_exists(dir: &Dir, path: &str) -> Result<bool, CliError> {
    match dir.metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(CliError::unverified(format!(
            "unverified: failed to inspect `{path}`: {err}"
        ))),
    }
}

fn list_files(dir: &Dir, path: &str) -> io::Result<Vec<String>> {
    let entries = dir.read_dir(path)?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(io::Error::other(format!(
                "path under `{path}` is not valid UTF-8"
            )));
        };
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        names.push(name.to_string());
    }
    Ok(names)
}

fn read_optional(dir: &Dir, path: &str) -> Result<Option<String>, CliError> {
    read_optional_before_open(dir, path, || {})
}

fn read_optional_before_open(
    dir: &Dir,
    path: &str,
    before_open: impl FnOnce(),
) -> Result<Option<String>, CliError> {
    match dir.metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(CliError::unverified(format!(
                "unverified: `{path}` is not a regular file"
            )));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CliError::unverified(format!(
                "unverified: failed to inspect `{path}`: {err}"
            )));
        }
    }
    before_open();
    let mut file = open_read_only(dir, path).map_err(|err| {
        CliError::unverified(format!("unverified: failed to open `{path}`: {err}"))
    })?;
    let metadata = file.metadata().map_err(|err| {
        CliError::unverified(format!("unverified: failed to inspect `{path}`: {err}"))
    })?;
    if !metadata.is_file() {
        return Err(CliError::unverified(format!(
            "unverified: `{path}` is not a regular file"
        )));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        CliError::unverified(format!("unverified: failed to read `{path}`: {err}"))
    })?;
    Ok(Some(text))
}

fn open_read_only(dir: &Dir, path: &str) -> io::Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    dir.open_with(path, &options)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use cap_std::ambient_authority;
    use cap_std::fs::Dir;

    use super::read_optional_before_open;

    fn fixture(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("oakum-detect-read-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("fixture root");
        root
    }

    fn open_root(root: &Path) -> Dir {
        Dir::open_ambient_dir(root, ambient_authority()).expect("repository capability")
    }

    #[test]
    fn raced_fifo_is_opened_nonblocking_and_rejected() {
        let root = fixture("raced-fifo-parent");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "cli::detect_tools::tests::raced_fifo_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("OAKUM_RACED_DETECT_FIFO_ROOT", &root)
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
                panic!("detect open blocked after a regular file became a FIFO");
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
        let root =
            PathBuf::from(std::env::var_os("OAKUM_RACED_DETECT_FIFO_ROOT").expect("fixture root"));
        let path = root.join("package.json");
        fs::write(&path, "{}").expect("regular package.json");
        let dir = open_root(&root);

        let result = read_optional_before_open(&dir, "package.json", || {
            fs::remove_file(&path).expect("remove regular package.json");
            let status = Command::new("mkfifo").arg(&path).status().expect("mkfifo");
            assert!(status.success(), "mkfifo: {status}");
        });

        let error = result.expect_err("FIFO must fail");
        assert!(error.to_string().contains("regular file"), "{error}");
    }
}
