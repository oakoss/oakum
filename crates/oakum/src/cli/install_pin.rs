//! Read-only verification of install pins (ADR-0007).
//!
//! Oakum does not write workflow files (ADR-0003). `check` scans
//! `.github/workflows`, the root `package.json`, and `.mise.toml` /
//! `mise.toml` for an exact oakum version and compares it to `tool-version`.
//! A missed look is `unverified`, not `ok`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use semver::Version;

use super::CliError;

pub(super) fn verify(dir: &Dir, expected: &Version) -> Result<(), Box<dyn std::error::Error>> {
    let pins = collect_pins(dir)?;
    if pins.is_empty() {
        return Err(Box::new(CliError::new(format!(
            "unverified: no oakum install pin in `.github/workflows`, `package.json`, \
             or `.mise.toml`; pin the same version as `tool-version` (`{expected}`), \
             for example `cargo binstall --no-confirm oakum@{expected}`"
        ))));
    }
    let mismatches: Vec<&FoundPin> = pins.iter().filter(|pin| pin.version != *expected).collect();
    if mismatches.is_empty() {
        return Ok(());
    }
    let listed = mismatches
        .iter()
        .map(|pin| format!("{} ({})", pin.version, pin.source.display()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(Box::new(CliError::new(format!(
        "unverified: install pin is {listed} but `tool-version` is `{expected}`"
    ))))
}

#[derive(Debug)]
struct FoundPin {
    source: PathBuf,
    version: Version,
}

fn collect_pins(dir: &Dir) -> Result<Vec<FoundPin>, Box<dyn std::error::Error>> {
    let mut pins = Vec::new();
    scan_workflows(dir, &mut pins)?;
    if let Some(pin) = read_package_json_pin(dir)? {
        pins.push(pin);
    }
    if let Some(pin) = read_mise_pin(dir)? {
        pins.push(pin);
    }
    Ok(pins)
}

fn scan_workflows(dir: &Dir, pins: &mut Vec<FoundPin>) -> Result<(), Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(".github/workflows") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "unverified: failed to read `.github/workflows`: {err}"
            ))));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::new(format!(
                "unverified: failed to read `.github/workflows`: {err}"
            ))
        })?;
        let name = entry.file_name();
        let path = Path::new(".github/workflows").join(&name);
        let Some(name) = name.to_str() else {
            return Err(Box::new(CliError::new(format!(
                "unverified: workflow path `{}` is not valid UTF-8",
                path.display()
            ))));
        };
        let is_yaml = Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"));
        if !is_yaml {
            continue;
        }
        let file_type = entry.file_type().map_err(|err| {
            CliError::new(format!(
                "unverified: failed to inspect `{}`: {err}",
                path.display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(Box::new(CliError::new(format!(
                "unverified: `{}` is not a file",
                path.display()
            ))));
        }
        let text = read_text(dir, &path)?;
        for version in versions_in_workflow(&text).map_err(|raw| {
            if raw == "unversioned" {
                CliError::new(format!(
                    "unverified: `{}` installs oakum without a version",
                    path.display()
                ))
            } else {
                CliError::new(format!(
                    "unverified: `{}` pins oakum as `{raw}`, which is not an exact version",
                    path.display()
                ))
            }
        })? {
            pins.push(FoundPin {
                source: path.clone(),
                version,
            });
        }
    }
    Ok(())
}

fn read_text(dir: &Dir, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = dir.open(path).map_err(|err| {
        CliError::new(format!(
            "unverified: failed to read `{}`: {err}",
            path.display()
        ))
    })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        CliError::new(format!(
            "unverified: failed to read `{}`: {err}",
            path.display()
        ))
    })?;
    Ok(text)
}

fn read_package_json_pin(dir: &Dir) -> Result<Option<FoundPin>, Box<dyn std::error::Error>> {
    let text = match dir.open("package.json") {
        Ok(mut file) => {
            let mut text = String::new();
            file.read_to_string(&mut text).map_err(|err| {
                CliError::new(format!("unverified: failed to read `package.json`: {err}"))
            })?;
            text
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "unverified: failed to read `package.json`: {err}"
            ))));
        }
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        CliError::new(format!(
            "unverified: `package.json` is not valid JSON: {err}"
        ))
    })?;
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(
            "unverified: `package.json` is not a JSON object",
        )));
    };
    let mut pin = None;
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(section_value) = object.get(section) else {
            continue;
        };
        let Some(deps) = section_value.as_object() else {
            return Err(Box::new(CliError::new(format!(
                "unverified: `package.json` `{section}` is not an object"
            ))));
        };
        let Some(oakum) = deps.get("oakum") else {
            continue;
        };
        let Some(raw) = oakum.as_str() else {
            return Err(Box::new(CliError::new(
                "unverified: `package.json` pins oakum with a non-string value",
            )));
        };
        let Some(version) = exact_version(raw) else {
            return Err(Box::new(CliError::new(format!(
                "unverified: `package.json` pins oakum as `{raw}`, which is not an exact version"
            ))));
        };
        match &pin {
            None => pin = Some(version),
            Some(existing) if *existing != version => {
                return Err(Box::new(CliError::new(format!(
                    "unverified: `package.json` pins oakum as both `{existing}` and `{version}`"
                ))));
            }
            Some(_) => {}
        }
    }
    Ok(pin.map(|version| FoundPin {
        source: PathBuf::from("package.json"),
        version,
    }))
}

fn read_mise_pin(dir: &Dir) -> Result<Option<FoundPin>, Box<dyn std::error::Error>> {
    let mut found = None;
    for name in [".mise.toml", "mise.toml"] {
        let Some(pin) = read_one_mise(dir, name)? else {
            continue;
        };
        match &found {
            None => found = Some(pin),
            Some(existing) if existing.version != pin.version => {
                return Err(Box::new(CliError::new(format!(
                    "unverified: `{}` pins oakum as `{}` but `{}` pins `{pin_version}`",
                    existing.source.display(),
                    existing.version,
                    pin.source.display(),
                    pin_version = pin.version,
                ))));
            }
            Some(_) => {}
        }
    }
    Ok(found)
}

fn read_one_mise(dir: &Dir, name: &str) -> Result<Option<FoundPin>, Box<dyn std::error::Error>> {
    let text = match dir.open(name) {
        Ok(mut file) => {
            let mut text = String::new();
            file.read_to_string(&mut text).map_err(|err| {
                CliError::new(format!("unverified: failed to read `{name}`: {err}"))
            })?;
            text
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "unverified: failed to read `{name}`: {err}"
            ))));
        }
    };
    let value: toml::Value = toml::from_str(&text)
        .map_err(|err| CliError::new(format!("unverified: `{name}` is not valid TOML: {err}")))?;
    let Some(tools_value) = value.get("tools") else {
        return Ok(None);
    };
    let Some(tools) = tools_value.as_table() else {
        return Err(Box::new(CliError::new(format!(
            "unverified: `{name}` has a `tools` value that is not a table"
        ))));
    };
    let mut pin = None;
    for (key, spec) in tools {
        if !is_mise_oakum_key(key) {
            continue;
        }
        let raw = mise_version_spec(spec).ok_or_else(|| {
            CliError::new(format!(
                "unverified: `{name}` pins `{key}` with a value that is not an exact version"
            ))
        })?;
        let Some(version) = exact_version(raw) else {
            return Err(Box::new(CliError::new(format!(
                "unverified: `{name}` pins oakum as `{raw}`, which is not an exact version"
            ))));
        };
        match &pin {
            None => pin = Some(version),
            Some(existing) if *existing != version => {
                return Err(Box::new(CliError::new(format!(
                    "unverified: `{name}` pins oakum as both `{existing}` and `{version}`"
                ))));
            }
            Some(_) => {}
        }
    }
    Ok(pin.map(|version| FoundPin {
        source: PathBuf::from(name),
        version,
    }))
}

fn is_mise_oakum_key(key: &str) -> bool {
    key == "oakum" || key == "cargo:oakum"
}

fn mise_version_spec(spec: &toml::Value) -> Option<&str> {
    spec.as_str()
        .or_else(|| spec.get("version").and_then(toml::Value::as_str))
}

fn versions_in_workflow(text: &str) -> Result<Vec<Version>, String> {
    let mut versions = Vec::new();
    for line in expand_block_scalars(&logical_lines(text)) {
        let code = strip_comment(&line);
        if !is_install_line(code) {
            continue;
        }
        for unit in install_units(code) {
            if unversioned_oakum_install(unit) {
                return Err("unversioned".to_owned());
            }
            for raw in oakum_at_specs(unit) {
                match exact_version(raw) {
                    Some(version) => versions.push(version),
                    None => return Err(raw.to_owned()),
                }
            }
            for raw in cargo_install_specs(unit) {
                match exact_version(raw) {
                    Some(version) => versions.push(version),
                    None => return Err(raw.to_owned()),
                }
            }
        }
    }
    Ok(versions)
}

fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let trimmed = line.trim_end();
        if let Some(prefix) = trimmed.strip_suffix('\\') {
            pending.push_str(prefix.trim_end());
            pending.push(' ');
            continue;
        }
        pending.push_str(trimmed);
        lines.push(std::mem::take(&mut pending));
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

fn expand_block_scalars(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        let indent = line.len() - line.trim_start().len();
        if let Some((key, folded)) = block_scalar_key(line.trim_start()) {
            index += 1;
            let mut body_lines = Vec::new();
            while index < lines.len() {
                let body = &lines[index];
                if body.trim().is_empty() {
                    index += 1;
                    continue;
                }
                let body_indent = body.len() - body.trim_start().len();
                if body_indent <= indent {
                    break;
                }
                body_lines.push(body.trim().to_owned());
                index += 1;
            }
            if folded {
                out.push(format!("{key}: {}", body_lines.join(" ")));
            } else {
                for body in body_lines {
                    out.push(format!("{key}: {body}"));
                }
            }
            continue;
        }
        out.push(line.clone());
        index += 1;
    }
    out
}

fn block_scalar_key(trimmed: &str) -> Option<(&'static str, bool)> {
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    for key in ["run", "tool"] {
        let Some(rest) = trimmed
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
        else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with('|') {
            return Some((key, false));
        }
        if rest.starts_with('>') {
            return Some((key, true));
        }
    }
    None
}

fn is_install_line(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let trimmed = lowered.trim();
    if let Some(tool) = trimmed
        .strip_prefix("tool:")
        .or_else(|| trimmed.strip_prefix("- tool:"))
    {
        return token_present(tool, "oakum");
    }
    if let Some(uses) = trimmed
        .strip_prefix("uses:")
        .or_else(|| trimmed.strip_prefix("- uses:"))
    {
        return uses.contains("install-action");
    }
    let Some(command) = run_command(&lowered) else {
        return false;
    };
    shell_segments(command).iter().any(|segment| {
        if is_echo_or_printf(segment) {
            return false;
        }
        segment.contains("binstall") || segment.contains("cargo install")
    })
}

fn install_units(line: &str) -> Vec<&str> {
    let Some(command) = run_command(line) else {
        return vec![line];
    };
    shell_segments(command)
        .into_iter()
        .filter(|segment| !is_echo_or_printf(segment))
        .collect()
}

fn shell_segments(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut rest = command;
    loop {
        let lower = rest.to_ascii_lowercase();
        let split = ["&&", ";"]
            .iter()
            .filter_map(|sep| lower.find(sep).map(|index| (index, sep.len())))
            .min_by_key(|(index, _)| *index);
        if let Some((index, sep_len)) = split {
            segments.push(rest[..index].trim());
            rest = rest[index + sep_len..].trim();
        } else {
            segments.push(rest.trim());
            break;
        }
    }
    segments
}

fn is_echo_or_printf(segment: &str) -> bool {
    let trimmed = segment.trim().to_ascii_lowercase();
    trimmed.starts_with("echo ")
        || trimmed.starts_with("echo\"")
        || trimmed.starts_with("echo'")
        || trimmed.starts_with("printf ")
}

fn run_command(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    let prefix_len = if lower.starts_with("- run:") {
        "- run:".len()
    } else if lower.starts_with("run:") {
        "run:".len()
    } else {
        return None;
    };
    let command = trimmed[prefix_len..].trim();
    (!command.is_empty()).then_some(command)
}

/// Cut at `#` outside quotes so `oakum@1.0.0 # pin` still yields the version.
fn strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, character) in line.char_indices() {
        match character {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return line[..index].trim_end(),
            _ => {}
        }
    }
    line
}

fn oakum_at_specs(line: &str) -> Vec<&str> {
    let lowered = line.to_ascii_lowercase();
    let mut specs = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = lowered[cursor..].find("oakum@") {
        let start = cursor + relative;
        if !is_token_start(line, start) {
            cursor = start + 6;
            continue;
        }
        specs.push(version_token(&line[start + 6..]));
        cursor = start + 6;
    }
    specs
}

fn unversioned_oakum_install(line: &str) -> bool {
    unversioned_tool_line(line) || unversioned_binstall(line) || unversioned_cargo_install(line)
}

fn unversioned_tool_line(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let trimmed = lowered.trim();
    let Some(tool) = trimmed
        .strip_prefix("tool:")
        .or_else(|| trimmed.strip_prefix("- tool:"))
    else {
        return false;
    };
    tool.split([',', ' ', '\t'])
        .map(crate_token)
        .any(|token| token == "oakum")
}

fn unversioned_binstall(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let mut search = 0;
    while let Some(relative) = lowered[search..].find("binstall") {
        let start = search + relative;
        let rest = &line[start + "binstall".len()..];
        if binstall_oakum_unversioned(rest) {
            return true;
        }
        search = start + "binstall".len();
    }
    false
}

fn binstall_oakum_unversioned(rest: &str) -> bool {
    let lower_rest = rest.to_ascii_lowercase();
    let end = ["&&", ";", "|"]
        .iter()
        .filter_map(|sep| lower_rest.find(sep))
        .min()
        .unwrap_or(rest.len());
    argv_oakum_unversioned(rest[..end].trim())
}

fn crate_token(token: &str) -> &str {
    token.trim_matches(|character| {
        matches!(
            character,
            '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '`' | ',' | ';'
        )
    })
}

fn unversioned_cargo_install(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let mut search = 0;
    while let Some(relative) = lowered[search..].find("cargo install") {
        let start = search + relative;
        let rest = &line[start + "cargo install".len()..];
        if cargo_install_oakum_unversioned(rest) {
            return true;
        }
        search = start + "cargo install".len();
    }
    false
}

fn cargo_install_oakum_unversioned(rest: &str) -> bool {
    let lower_rest = rest.to_ascii_lowercase();
    let end = ["&&", ";", "|"]
        .iter()
        .filter_map(|sep| lower_rest.find(sep))
        .min()
        .unwrap_or(rest.len());
    argv_oakum_unversioned(rest[..end].trim())
}

fn argv_oakum_unversioned(segment: &str) -> bool {
    let mut saw_oakum = false;
    let mut version = false;
    let mut tokens = segment.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let lower = crate_token(token).to_ascii_lowercase();
        if lower.starts_with("oakum@") {
            continue;
        }
        if lower == "oakum" {
            saw_oakum = true;
            continue;
        }
        if let Some(spec) = lower
            .strip_prefix("--version=")
            .or_else(|| lower.strip_prefix("--vers="))
        {
            version = exact_version(spec).is_some();
            continue;
        }
        if lower == "--version" || lower == "--vers" {
            version = tokens
                .next()
                .and_then(|next| exact_version(crate_token(next)))
                .is_some();
        }
    }
    saw_oakum && !version
}

fn cargo_install_specs(line: &str) -> Vec<&str> {
    let lowered = line.to_ascii_lowercase();
    let mut specs = Vec::new();
    let mut search = 0;
    while let Some(relative) = lowered[search..].find("cargo install") {
        let start = search + relative;
        if let Some(spec) = cargo_install_spec_from(&line[start + "cargo install".len()..]) {
            specs.push(spec);
        }
        search = start + "cargo install".len();
    }
    specs
}

fn cargo_install_spec_from(rest: &str) -> Option<&str> {
    let lower_rest = rest.to_ascii_lowercase();
    let end = ["&&", ";", "|"]
        .iter()
        .filter_map(|sep| lower_rest.find(sep))
        .min()
        .unwrap_or(rest.len());
    let segment = rest[..end].trim();
    let mut saw_oakum = false;
    let mut version = None;
    let mut tokens = segment.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let token = crate_token(token);
        let lower = token.to_ascii_lowercase();
        if let Some(spec) = lower.strip_prefix("oakum@") {
            return Some(version_token(&token[token.len() - spec.len()..]));
        }
        if lower == "oakum" {
            saw_oakum = true;
            continue;
        }
        if let Some(spec) = lower
            .strip_prefix("--version=")
            .or_else(|| lower.strip_prefix("--vers="))
        {
            version = Some(version_token(&token[token.len() - spec.len()..]));
            continue;
        }
        if lower == "--version" || lower == "--vers" {
            if let Some(next) = tokens.next() {
                version = Some(version_token(
                    next.trim_matches(|character| matches!(character, '"' | '\'')),
                ));
            }
        }
    }
    saw_oakum.then_some(())?;
    version
}

fn token_present(lowered: &str, token: &str) -> bool {
    let mut cursor = 0;
    while let Some(relative) = lowered[cursor..].find(token) {
        let start = cursor + relative;
        if is_token_start(lowered, start) && is_token_end(lowered, start + token.len()) {
            return true;
        }
        cursor = start + 1;
    }
    false
}

fn is_token_start(text: &str, index: usize) -> bool {
    index == 0 || {
        let previous = text.as_bytes()[index - 1];
        !previous.is_ascii_alphanumeric() && previous != b'_' && previous != b'-'
    }
}

fn is_token_end(text: &str, index: usize) -> bool {
    index >= text.len() || {
        let next = text.as_bytes()[index];
        !next.is_ascii_alphanumeric() && next != b'_' && next != b'-'
    }
}

fn version_token(rest: &str) -> &str {
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | ',' | ';' | '#' | '|' | '`' | ')' | ']'
                )
        })
        .unwrap_or(rest.len());
    rest[..end].trim_end_matches(['"', '\''])
}

fn exact_version(raw: &str) -> Option<Version> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(text: &str) -> Vec<Version> {
        versions_in_workflow(text).expect("exact pins")
    }

    #[test]
    fn binstall_line_yields_the_version() {
        let versions = workflow("run: cargo binstall --no-confirm oakum@1.2.3\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
    }

    #[test]
    fn v_prefix_and_comment_are_stripped() {
        let versions = workflow("run: cargo binstall oakum@v0.1.0 # pin\n");
        assert_eq!(versions, vec![Version::parse("0.1.0").unwrap()]);
    }

    #[test]
    fn double_v_prefix_is_not_an_exact_pin() {
        let err = versions_in_workflow("run: cargo binstall oakum@vv0.1.0\n").unwrap_err();
        assert_eq!(err, "vv0.1.0");
    }

    #[test]
    fn commented_out_pin_is_ignored() {
        assert!(workflow("# cargo binstall oakum@1.2.3\n").is_empty());
    }

    #[test]
    fn cargo_install_version_flag() {
        let versions = workflow("run: cargo install oakum --version 2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
        let versions = workflow("run: cargo install oakum --vers=2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
    }

    #[test]
    fn cargo_install_continuation_is_one_command() {
        let versions = workflow("run: cargo install oakum \\\n  --version 0.0.0\n");
        assert_eq!(versions, vec![Version::parse("0.0.0").unwrap()]);
    }

    #[test]
    fn run_block_scalar_is_an_install() {
        let versions = workflow("run: |\n  cargo binstall --no-confirm oakum@1.2.3\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
    }

    #[test]
    fn inexact_pin_in_run_block_is_unverified_even_when_another_pin_matches() {
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0\nrun: |\n  cargo binstall oakum@latest\n",
        )
        .unwrap_err();
        assert_eq!(err, "latest");
    }

    #[test]
    fn inexact_pin_in_tool_block_is_unverified_even_when_another_pin_matches() {
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0\n        tool: |\n          oakum@latest\n",
        )
        .unwrap_err();
        assert_eq!(err, "latest");
    }

    #[test]
    fn cargo_install_version_must_belong_to_oakum() {
        assert!(workflow("run: cargo install ripgrep --version 0.0.0 && oakum check\n").is_empty());
    }

    #[test]
    fn env_or_echo_oakum_at_is_not_a_pin() {
        assert!(workflow("env:\n  OLD: oakum@0.0.0\n").is_empty());
        assert!(workflow("run: echo oakum@0.0.0\n").is_empty());
        assert!(workflow("run: echo cargo binstall oakum@0.0.0\n").is_empty());
        assert!(workflow("OLD: cargo binstall oakum@0.0.0\n").is_empty());
    }

    #[test]
    fn install_action_tool_line_is_a_pin() {
        let versions = workflow("        tool: oakum@0.3.1\n");
        assert_eq!(versions, vec![Version::parse("0.3.1").unwrap()]);
    }

    #[test]
    fn tool_block_scalar_is_a_pin() {
        let versions = workflow("        tool: |\n          oakum@0.3.1\n");
        assert_eq!(versions, vec![Version::parse("0.3.1").unwrap()]);
    }

    #[test]
    fn folded_run_block_is_one_command() {
        let versions = workflow("run: >\n  cargo install oakum\n  --version 0.0.0\n");
        assert_eq!(versions, vec![Version::parse("0.0.0").unwrap()]);
    }

    #[test]
    fn every_cargo_install_on_a_line_is_scanned() {
        let versions = workflow(
            "run: cargo install oakum --version 0.0.0 && cargo install oakum --version 1.2.3\n",
        );
        assert_eq!(
            versions,
            vec![
                Version::parse("0.0.0").unwrap(),
                Version::parse("1.2.3").unwrap()
            ]
        );
    }

    #[test]
    fn unversioned_binstall_is_unverified() {
        let err = versions_in_workflow("run: cargo binstall oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let err =
            versions_in_workflow("run: cargo binstall oakum@0.0.0\nrun: cargo binstall oakum\n")
                .unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn unversioned_install_on_the_same_line_is_unverified() {
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0 && cargo binstall oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn unversioned_tool_line_is_unverified_even_when_another_pin_matches() {
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0\n        tool: oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn echo_then_binstall_is_a_pin() {
        let versions =
            workflow("run: echo installing && cargo binstall --no-confirm oakum@0.0.0\n");
        assert_eq!(versions, vec![Version::parse("0.0.0").unwrap()]);
    }

    #[test]
    fn echoed_oakum_at_is_not_a_pin() {
        assert!(
            workflow("run: echo oakum@0.0.0 && cargo binstall --no-confirm ripgrep\n").is_empty()
        );
    }

    #[test]
    fn unversioned_binstall_target_after_a_pin_is_unverified() {
        let err = versions_in_workflow("run: cargo binstall oakum@0.0.0 oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let err = versions_in_workflow("run: cargo binstall ripgrep oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let err = versions_in_workflow("run: (cargo binstall oakum@0.0.0 oakum)\n").unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn unversioned_cargo_install_target_after_a_pin_is_unverified() {
        let err = versions_in_workflow(
            "run: cargo install oakum --version 0.0.0 && cargo install oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, "unversioned");
        let err = versions_in_workflow("run: cargo install oakum@0.0.0 oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let err = versions_in_workflow("run: cargo install ripgrep oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let versions = workflow("run: cargo install --version 1.2.3 ripgrep oakum\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
        let versions = workflow("run: cargo install ripgrep oakum --version 1.2.3\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
        let err = versions_in_workflow("run: cargo install oakum --version\n").unwrap_err();
        assert_eq!(err, "unversioned");
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0 && cargo install oakum --versioned\n",
        )
        .unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn unversioned_tool_entry_after_a_pin_is_unverified() {
        let err = versions_in_workflow("        tool: oakum@0.0.0, oakum\n").unwrap_err();
        assert_eq!(err, "unversioned");
    }

    #[test]
    fn bare_oakum_invocation_is_not_a_pin() {
        assert!(workflow("run: oakum check\n").is_empty());
    }

    #[test]
    fn not_oakum_at_is_ignored() {
        assert!(workflow("run: cargo binstall fake-oakum@1.0.0\n").is_empty());
    }

    #[test]
    fn range_or_latest_is_unverified() {
        let err = versions_in_workflow("run: cargo binstall oakum@latest\n").unwrap_err();
        assert_eq!(err, "latest");
        let err = versions_in_workflow("run: cargo binstall oakum@^1.0.0\n").unwrap_err();
        assert_eq!(err, "^1.0.0");
    }

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("oakum-pin-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn package_json_later_section_cannot_hide_an_inexact_pin() {
        let root = scratch("pkg-conflict");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"devDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest in later section");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn package_json_peer_section_cannot_hide_an_inexact_pin() {
        let root = scratch("pkg-peer");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"peerDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("peer latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn package_json_range_is_not_an_exact_pin() {
        let root = scratch("pkg-range");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"^0.1.0"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("range");
        assert!(err.to_string().contains("not an exact version"), "{err}");
        assert!(err.to_string().contains("^0.1.0"), "{err}");
    }

    #[test]
    fn inexact_pin_is_unverified_even_when_another_pin_matches() {
        let root = scratch("latest-and-exact");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".github/workflows/ci.yml"),
            "run: cargo binstall --no-confirm oakum@latest\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest");
        assert!(err.to_string().contains("latest"), "{err}");
        assert!(err.to_string().contains("not an exact version"), "{err}");
    }

    #[test]
    fn invalid_package_json_is_unverified() {
        let root = scratch("pkg-invalid");
        std::fs::write(root.join("package.json"), "{").unwrap();
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("invalid json");
        assert!(err.to_string().contains("not valid JSON"), "{err}");
    }

    #[test]
    fn package_json_exact_pin_is_collected() {
        let root = scratch("pkg-exact");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"0.4.2"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].source, PathBuf::from("package.json"));
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn package_json_conflicting_exact_versions_are_unverified() {
        let root = scratch("pkg-both-exact");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"devDependencies":{"oakum":"1.0.0"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("conflicting exacts");
        assert!(err.to_string().contains("both"), "{err}");
    }

    #[test]
    fn package_json_non_string_pin_is_unverified() {
        let root = scratch("pkg-object");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":{"version":"0.0.0"}}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("non-string");
        assert!(err.to_string().contains("non-string"), "{err}");
    }

    #[test]
    fn package_json_optional_section_inexact_pin_is_unverified() {
        let root = scratch("pkg-optional");
        std::fs::write(
            root.join("package.json"),
            r#"{"optionalDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("optional latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn mise_exact_pin_is_collected() {
        let root = scratch("mise-exact");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"0.4.2\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].source, PathBuf::from(".mise.toml"));
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn mise_cargo_backend_and_table_form_are_pins() {
        let root = scratch("mise-cargo");
        std::fs::write(
            root.join("mise.toml"),
            "[tools]\n\"cargo:oakum\" = { version = \"0.1.0\" }\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins[0].source, PathBuf::from("mise.toml"));
        assert_eq!(pins[0].version, Version::parse("0.1.0").unwrap());
    }

    #[test]
    fn mise_latest_is_unverified() {
        let root = scratch("mise-latest");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"latest\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn mise_conflicting_files_are_unverified() {
        let root = scratch("mise-conflict");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"0.1.0\"\n").unwrap();
        std::fs::write(root.join("mise.toml"), "[tools]\noakum = \"0.2.0\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("conflict");
        assert!(err.to_string().contains("0.1.0"), "{err}");
        assert!(err.to_string().contains("0.2.0"), "{err}");
    }

    #[test]
    fn mise_tools_that_is_not_a_table_is_unverified() {
        let root = scratch("mise-tools-string");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        std::fs::write(root.join(".mise.toml"), "tools = \"oakum@latest\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("tools not a table");
        assert!(err.to_string().contains("not a table"), "{err}");
    }

    fn matching_workflow(root: &Path) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
    }

    #[test]
    fn yaml_named_directory_is_unverified() {
        let root = scratch("yaml-dir");
        matching_workflow(&root);
        std::fs::create_dir_all(root.join(".github/workflows/ci.yml")).unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("yaml directory");
        assert!(err.to_string().contains("not a file"), "{err}");
    }

    #[test]
    fn package_json_non_object_root_is_unverified() {
        let root = scratch("pkg-array");
        matching_workflow(&root);
        std::fs::write(root.join("package.json"), "[]").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("array root");
        assert!(err.to_string().contains("not a JSON object"), "{err}");
    }

    #[test]
    fn package_json_non_object_section_is_unverified() {
        let root = scratch("pkg-section-string");
        matching_workflow(&root);
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":"oakum@latest"}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("section string");
        assert!(err.to_string().contains("not an object"), "{err}");
    }
}
