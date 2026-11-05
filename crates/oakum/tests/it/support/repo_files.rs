//! The `.changeset` and install-pin files a fixture writes, and the versions
//! they carry.

use std::path::Path;

pub const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The stderr line every config-loading command but `upgrade` prints while the
/// retired key is present (ADR-0038).
pub const TOOL_VERSION_WARNING: &str = "`.changeset/_config.toml` sets `tool-version`, which oakum no longer reads; run `oakum upgrade` to remove it";

/// An empty `_config.toml` plus a mise pin at the binary under test, the
/// least that `check` and `release` accept (ADR-0007, ADR-0038).
pub fn pinned_config(root: &Path) {
    write_config(root, "");
    std::fs::write(
        root.join(".mise.toml"),
        format!("[tools]\n\"cargo:oakum\" = \"{BINARY_VERSION}\"\n"),
    )
    .expect("mise pin");
}

pub fn write_config(root: &Path, body: &str) {
    std::fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    std::fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

/// `.changeset/one.md` bumping `name` by `level`.
pub fn write_changeset(root: &Path, name: &str, level: &str) {
    std::fs::create_dir_all(root.join(".changeset")).expect("changeset");
    std::fs::write(
        root.join(".changeset/one.md"),
        format!("---\n{name}: {level}\n---\n\n{level} {name}\n"),
    )
    .expect("changeset");
}

pub fn write_patch_changeset(root: &Path, name: &str) {
    write_changeset(root, name, "patch");
}

pub fn write_install_pin(root: &Path, version: &str) {
    std::fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    std::fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: cargo binstall --no-confirm oakum@{version}\n"),
    )
    .expect("workflow");
}

/// A workflow line, which mise never reads, rather than a `.mise.toml` a mise
/// shim spawned in the fixture would. Call before the fixture's first commit.
pub fn pin_oakum(root: &Path) {
    write_install_pin(root, BINARY_VERSION);
}

/// What a patch bump makes of `version`: a self-host fixture's `oakum` member
/// sits at the binary's version, so its bumped version moves with the binary.
pub fn next_patch(version: &str) -> String {
    let mut next = semver::Version::parse(version).expect("semver");
    next.patch += 1;
    next.to_string()
}
