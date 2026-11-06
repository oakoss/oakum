//! The exact oakum versions a workflow or composite action pins, read from its
//! text: YAML walked, shell lines split, no file read.

use semver::Version;
use serde_json::Value;

use super::NPM_PACKAGE;

/// Why a workflow gave no exact pin.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::cli) enum PinError {
    Unversioned,
    UnparseableYaml,
    WithNotObject,
    /// A spec that is not an exact version, as written.
    Inexact(String),
}

pub(in crate::cli) fn versions_in_workflow(text: &str) -> Result<Vec<Version>, PinError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_saphyr::from_str(text).map_err(|_| PinError::UnparseableYaml)?;
    let mut versions = Vec::new();
    walk_workflow_value(&value, &mut versions)?;
    Ok(versions)
}

fn walk_workflow_value(value: &Value, versions: &mut Vec<Version>) -> Result<(), PinError> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(run)) = map.get("run") {
                append_run_versions(run, versions)?;
            }
            if let Some(tool) = map.get("tool") {
                append_tool_value(tool, versions)?;
            }
            if let Some(with) = map.get("with") {
                walk_with_value(with, versions)?;
            }
            if let Some(matrix) = map.get("matrix") {
                walk_matrix_value(matrix, versions)?;
            }
            if let Some(Value::String(default)) = map.get("default") {
                append_run_versions(default, versions)?;
            }
            for (key, child) in map {
                if matches!(key.as_str(), "with" | "matrix") {
                    continue;
                }
                walk_workflow_value(child, versions)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_workflow_value(item, versions)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn walk_with_value(with: &Value, versions: &mut Vec<Version>) -> Result<(), PinError> {
    let Value::Object(map) = with else {
        return Err(PinError::WithNotObject);
    };
    for (key, child) in map {
        if key == "tool" {
            append_tool_value(child, versions)?;
        } else {
            append_stringish_as_run(child, versions)?;
        }
    }
    Ok(())
}

/// Owns the whole `matrix` subtree so `run:` cells are not also visited by the
/// recursive walker (which would double-count).
fn walk_matrix_value(matrix: &Value, versions: &mut Vec<Version>) -> Result<(), PinError> {
    match matrix {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "tool" {
                    append_tool_value(child, versions)?;
                } else {
                    match child {
                        Value::String(text) => append_run_versions(text, versions)?,
                        Value::Array(_) | Value::Object(_) => walk_matrix_value(child, versions)?,
                        _ => {}
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_matrix_value(item, versions)?;
            }
        }
        Value::String(text) => append_run_versions(text, versions)?,
        _ => {}
    }
    Ok(())
}

fn append_tool_value(value: &Value, versions: &mut Vec<Version>) -> Result<(), PinError> {
    match value {
        Value::String(tool) => append_tool_versions(tool, versions),
        Value::Array(items) => {
            for item in items {
                if let Value::String(tool) = item {
                    append_tool_versions(tool, versions)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn append_stringish_as_run(value: &Value, versions: &mut Vec<Version>) -> Result<(), PinError> {
    match value {
        Value::String(text) => append_run_versions(text, versions),
        Value::Array(items) => {
            for item in items {
                if let Value::String(text) = item {
                    append_run_versions(text, versions)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn append_run_versions(run: &str, versions: &mut Vec<Version>) -> Result<(), PinError> {
    for line in logical_lines(run) {
        if line.trim().is_empty() {
            continue;
        }
        append_install_line(&format!("run: {line}"), versions)?;
    }
    Ok(())
}

fn append_tool_versions(tool: &str, versions: &mut Vec<Version>) -> Result<(), PinError> {
    for line in logical_lines(tool) {
        if line.trim().is_empty() {
            continue;
        }
        append_install_line(&format!("tool: {line}"), versions)?;
    }
    Ok(())
}

fn append_install_line(line: &str, versions: &mut Vec<Version>) -> Result<(), PinError> {
    let code = strip_comment(line);
    if !is_install_line(code) {
        return Ok(());
    }
    for unit in install_units(code) {
        if unversioned_oakum_install(unit) {
            return Err(PinError::Unversioned);
        }
        for raw in install_version_specs(unit) {
            match exact_version(raw) {
                Some(version) => versions.push(version),
                None => return Err(PinError::Inexact(raw.to_owned())),
            }
        }
    }
    Ok(())
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

fn is_install_line(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let trimmed = lowered.trim();
    if let Some(tool) = trimmed
        .strip_prefix("tool:")
        .or_else(|| trimmed.strip_prefix("- tool:"))
    {
        return token_present(tool, "oakum");
    }
    let Some(command) = run_command(&lowered) else {
        return false;
    };
    shell_segments(command).iter().any(|segment| {
        if is_echo_or_printf(segment) {
            return false;
        }
        segment.contains("binstall")
            || segment.contains("cargo install")
            || is_npm_install_unit(segment)
    })
}

/// The package specs an npm-channel install unit names, or `None` when the
/// segment is not one. `npm i` / `npm install` / `npm add` and `pnpm add` /
/// `pnpm install` / `pnpm i` take every positional as a package; `npx` and
/// `pnpm dlx` take only the first (or `-p` / `--package`), the rest being
/// the command's own argv. `pnpm exec oakum` is an invocation. A leading
/// `(`, `sudo` and its flags, `VAR=value`, and manager options before the
/// subcommand (`pnpm -w add`, `npm --prefix . i`) do not change which
/// command runs, so they are skipped; `--filter`, `--prefix`, `--dir`,
/// `-C`, and `-F` take a value.
fn npm_install_specs(segment: &str) -> Option<Vec<&str>> {
    let mut argv = segment
        .split_whitespace()
        .map(|token| token.trim_start_matches('('))
        .filter(|token| !token.is_empty())
        .skip_while(|token| {
            token.eq_ignore_ascii_case("sudo")
                || token.starts_with('-')
                || (token.contains('=') && !token.starts_with('-'))
        })
        .map(crate_token);
    let verb = argv.next()?.to_ascii_lowercase();
    let mut argv = argv.peekable();
    let single = match verb.as_str() {
        "npx" => true,
        "npm" => match next_subcommand(&mut argv)?.as_str() {
            "i" | "install" | "add" => false,
            _ => return None,
        },
        "pnpm" => match next_subcommand(&mut argv)?.as_str() {
            "dlx" => true,
            "add" | "install" | "i" => false,
            _ => return None,
        },
        _ => return None,
    };
    let mut specs = Vec::new();
    let mut positional_taken = false;
    let mut argv = argv.peekable();
    while let Some(token) = argv.next() {
        if let Some(spec) = token.strip_prefix("--package=") {
            specs.push(spec);
        } else if token == "-p" || token == "--package" {
            if let Some(spec) = argv.next() {
                specs.push(spec);
            }
        } else if token.starts_with('-') {
        } else if single {
            if !positional_taken {
                specs.push(token);
            }
            positional_taken = true;
        } else {
            specs.push(token);
        }
    }
    Some(specs)
}

fn next_subcommand<'a>(argv: &mut impl Iterator<Item = &'a str>) -> Option<String> {
    loop {
        let token = argv.next()?;
        if matches!(token, "--filter" | "--prefix" | "--dir" | "-C" | "-F") {
            argv.next();
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        return Some(token.to_ascii_lowercase());
    }
}

fn is_oakum_npm_spec(spec: &str) -> bool {
    spec.eq_ignore_ascii_case(NPM_PACKAGE)
        || spec
            .get(..NPM_PACKAGE.len() + 1)
            .is_some_and(|head| head.eq_ignore_ascii_case("@oakoss/oakum@"))
}

fn is_npm_install_unit(segment: &str) -> bool {
    npm_install_specs(segment).is_some_and(|specs| specs.iter().any(|spec| is_oakum_npm_spec(spec)))
}

fn npm_oakum_versions(segment: &str) -> Vec<&str> {
    npm_install_specs(segment)
        .unwrap_or_default()
        .into_iter()
        .filter(|spec| is_oakum_npm_spec(spec))
        .filter_map(|spec| spec.get(NPM_PACKAGE.len() + 1..))
        .map(version_token)
        .collect()
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

fn is_tool_install_unit(unit: &str) -> bool {
    let trimmed = unit.trim().to_ascii_lowercase();
    trimmed.starts_with("tool:") || trimmed.starts_with("- tool:")
}

fn install_version_specs(unit: &str) -> Vec<&str> {
    if is_tool_install_unit(unit) {
        return oakum_at_specs(unit);
    }
    let lowered = unit.to_ascii_lowercase();
    let mut specs = Vec::new();
    if lowered.contains("binstall") {
        specs.extend(binstall_specs(unit));
    }
    if lowered.contains("cargo install") {
        specs.extend(cargo_install_specs(unit));
    }
    if is_npm_install_unit(unit) {
        specs.extend(npm_oakum_versions(unit));
    }
    if specs.is_empty() {
        specs = oakum_at_specs(unit);
    }
    specs
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
    unversioned_tool_line(line)
        || unversioned_binstall(line)
        || unversioned_cargo_install(line)
        || unversioned_npm_install(line)
}

/// The bare package name resolves to the latest release, which is the drift
/// the pin exists to prevent.
fn unversioned_npm_install(line: &str) -> bool {
    npm_install_specs(line).is_some_and(|specs| {
        specs
            .iter()
            .any(|spec| spec.eq_ignore_ascii_case(NPM_PACKAGE))
    })
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
        // A value that is not an exact version is still a version:
        // `argv_oakum_version_spec` reports it, so this is not unversioned.
        if lower.starts_with("--version=") || lower.starts_with("--vers=") {
            version = true;
            continue;
        }
        if lower == "--version" || lower == "--vers" {
            version = tokens.next_if(|next| !next.starts_with('-')).is_some();
        }
    }
    saw_oakum && !version
}

fn binstall_specs(line: &str) -> Vec<&str> {
    install_command_specs(line, "binstall")
}

fn cargo_install_specs(line: &str) -> Vec<&str> {
    install_command_specs(line, "cargo install")
}

fn install_command_specs<'a>(line: &'a str, verb: &str) -> Vec<&'a str> {
    let lowered = line.to_ascii_lowercase();
    let mut specs = Vec::new();
    let mut search = 0;
    while let Some(relative) = lowered[search..].find(verb) {
        let start = search + relative;
        if let Some(spec) = argv_oakum_version_spec(&line[start + verb.len()..]) {
            specs.push(spec);
        }
        search = start + verb.len();
    }
    specs
}

fn argv_oakum_version_spec(rest: &str) -> Option<&str> {
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
            if let Some(next) = tokens.next_if(|next| !next.starts_with('-')) {
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

pub(super) fn exact_version(raw: &str) -> Option<Version> {
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

    fn assert_no_pin(text: &str) {
        let versions = workflow(text);
        assert!(versions.is_empty(), "{text:?} read as {versions:?}");
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
        assert_eq!(err, PinError::Inexact(String::from("vv0.1.0")));
    }

    #[test]
    fn commented_out_pin_is_ignored() {
        assert_no_pin("# cargo binstall oakum@1.2.3\n");
    }

    #[test]
    fn cargo_install_version_flag() {
        let versions = workflow("run: cargo install oakum --version 2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
        let versions = workflow("run: cargo install oakum --vers=2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
    }

    #[test]
    fn cargo_install_oakum_at_is_one_pin_not_two() {
        let versions = workflow("run: cargo install oakum@1.2.3\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
    }

    #[test]
    fn binstall_version_flag() {
        let versions = workflow("run: cargo binstall oakum --version 2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
        let versions = workflow("run: cargo binstall --no-confirm oakum --vers=2.0.0\n");
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
    }

    #[test]
    fn install_action_without_tool_is_not_a_pin() {
        assert_no_pin("uses: oakoss/install-action@v1\n");
        assert_no_pin("- uses: oakoss/install-action@v1\n");
    }

    #[test]
    fn mixed_binstall_and_cargo_install_collect_both_pins() {
        let versions = workflow("run: cargo binstall oakum@1.0.0 || cargo install oakum@2.0.0\n");
        assert_eq!(
            versions,
            vec![
                Version::parse("1.0.0").unwrap(),
                Version::parse("2.0.0").unwrap()
            ]
        );
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
            "jobs:\n  ci:\n    steps:\n      - run: cargo binstall --no-confirm oakum@0.0.0\n      - run: |\n          cargo binstall oakum@latest\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Inexact(String::from("latest")));
    }

    #[test]
    fn inexact_pin_in_tool_block_is_unverified_even_when_another_pin_matches() {
        let err = versions_in_workflow(
            "jobs:\n  ci:\n    steps:\n      - run: cargo binstall --no-confirm oakum@0.0.0\n      - uses: taiki-e/install-action\n        with:\n          tool: |\n            oakum@latest\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Inexact(String::from("latest")));
    }

    #[test]
    fn cargo_install_version_must_belong_to_oakum() {
        assert_no_pin("run: cargo install ripgrep --version 0.0.0 && oakum check\n");
    }

    #[test]
    fn env_or_echo_oakum_at_is_not_a_pin() {
        assert_no_pin("env:\n  OLD: oakum@0.0.0\n");
        assert_no_pin("run: echo oakum@0.0.0\n");
        assert_no_pin("run: echo cargo binstall oakum@0.0.0\n");
        assert_no_pin("OLD: cargo binstall oakum@0.0.0\n");
    }

    #[test]
    fn with_tool_array_is_a_pin() {
        let versions = workflow(
            "- uses: taiki-e/install-action\n  with:\n    tool:\n      - oakum@0.3.1\n      - ripgrep\n",
        );
        assert_eq!(versions, vec![Version::parse("0.3.1").unwrap()]);
    }

    #[test]
    fn with_install_cmd_string_is_a_pin() {
        let versions = workflow(
            "- uses: ./local-install\n  with:\n    install_cmd: cargo binstall --no-confirm oakum@1.4.0\n",
        );
        assert_eq!(versions, vec![Version::parse("1.4.0").unwrap()]);
    }

    #[test]
    fn with_run_string_is_a_pin() {
        let versions = workflow(
            "- uses: ./local-install\n  with:\n    run: cargo binstall --no-confirm oakum@1.5.0\n",
        );
        assert_eq!(versions, vec![Version::parse("1.5.0").unwrap()]);
    }

    #[test]
    fn with_install_cmd_array_is_a_pin() {
        let versions = workflow(
            "- uses: ./local-install\n  with:\n    install_cmd:\n      - cargo binstall --no-confirm oakum@1.6.0\n",
        );
        assert_eq!(versions, vec![Version::parse("1.6.0").unwrap()]);
    }

    #[test]
    fn matrix_install_array_is_a_pin() {
        let versions = workflow(
            "strategy:\n  matrix:\n    install:\n      - cargo binstall --no-confirm oakum@2.2.0\n",
        );
        assert_eq!(versions, vec![Version::parse("2.2.0").unwrap()]);
    }

    #[test]
    fn matrix_include_run_key_is_one_pin_not_two() {
        let versions = workflow(
            "strategy:\n  matrix:\n    include:\n      - run: cargo binstall --no-confirm oakum@2.0.0\n",
        );
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
    }

    #[test]
    fn non_object_with_is_unverified() {
        let err = versions_in_workflow(
            "- uses: ./local-install\n  with: cargo binstall --no-confirm oakum@1.4.0\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::WithNotObject);
    }

    #[test]
    fn matrix_install_cell_is_a_pin() {
        let versions = workflow(
            "strategy:\n  matrix:\n    install: cargo binstall --no-confirm oakum@2.0.0\n",
        );
        assert_eq!(versions, vec![Version::parse("2.0.0").unwrap()]);
    }

    #[test]
    fn matrix_include_install_cell_is_a_pin() {
        let versions = workflow(
            "strategy:\n  matrix:\n    include:\n      - install: cargo binstall --no-confirm oakum@2.1.0\n",
        );
        assert_eq!(versions, vec![Version::parse("2.1.0").unwrap()]);
    }

    #[test]
    fn matrix_tool_key_is_still_a_pin() {
        let versions = workflow("strategy:\n  matrix:\n    tool: oakum@0.3.1\n");
        assert_eq!(versions, vec![Version::parse("0.3.1").unwrap()]);
    }

    #[test]
    fn workflow_call_input_default_install_is_a_pin() {
        let versions = workflow(
            "on:\n  workflow_call:\n    inputs:\n      install:\n        type: string\n        default: cargo binstall --no-confirm oakum@3.0.0\n",
        );
        assert_eq!(versions, vec![Version::parse("3.0.0").unwrap()]);
    }

    #[test]
    fn dynamic_matrix_run_expression_alone_is_not_a_pin() {
        // Static analysis cannot resolve ${{ matrix.install }}; put the command in the cell.
        assert_no_pin("run: ${{ matrix.install }}\n");
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
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow(
            "jobs:\n  ci:\n    steps:\n      - run: cargo binstall oakum@0.0.0\n      - run: cargo binstall oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn unversioned_install_on_the_same_line_is_unverified() {
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0 && cargo binstall oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn unversioned_tool_line_is_unverified_even_when_another_pin_matches() {
        let err = versions_in_workflow(
            "jobs:\n  ci:\n    steps:\n      - run: cargo binstall --no-confirm oakum@0.0.0\n      - uses: taiki-e/install-action\n        with:\n          tool: oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn echo_then_binstall_is_a_pin() {
        let versions =
            workflow("run: echo installing && cargo binstall --no-confirm oakum@0.0.0\n");
        assert_eq!(versions, vec![Version::parse("0.0.0").unwrap()]);
    }

    #[test]
    fn echoed_oakum_at_is_not_a_pin() {
        assert_no_pin("run: echo oakum@0.0.0 && cargo binstall --no-confirm ripgrep\n");
    }

    #[test]
    fn unversioned_binstall_target_after_a_pin_is_unverified() {
        let err = versions_in_workflow("run: cargo binstall oakum@0.0.0 oakum\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow("run: cargo binstall ripgrep oakum\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow("run: (cargo binstall oakum@0.0.0 oakum)\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn unversioned_cargo_install_target_after_a_pin_is_unverified() {
        let err = versions_in_workflow(
            "run: cargo install oakum --version 0.0.0 && cargo install oakum\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow("run: cargo install oakum@0.0.0 oakum\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow("run: cargo install ripgrep oakum\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let versions = workflow("run: cargo install --version 1.2.3 ripgrep oakum\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
        let versions = workflow("run: cargo install ripgrep oakum --version 1.2.3\n");
        assert_eq!(versions, vec![Version::parse("1.2.3").unwrap()]);
        let err = versions_in_workflow("run: cargo install oakum --version\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
        let err = versions_in_workflow(
            "run: cargo binstall --no-confirm oakum@0.0.0 && cargo install oakum --versioned\n",
        )
        .unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn unversioned_tool_entry_after_a_pin_is_unverified() {
        let err = versions_in_workflow("        tool: oakum@0.0.0, oakum\n").unwrap_err();
        assert_eq!(err, PinError::Unversioned);
    }

    #[test]
    fn bare_oakum_invocation_is_not_a_pin() {
        assert_no_pin("run: oakum check\n");
    }

    #[test]
    fn not_oakum_at_is_ignored() {
        assert_no_pin("run: cargo binstall fake-oakum@1.0.0\n");
    }

    #[test]
    fn range_or_latest_is_unverified() {
        let err = versions_in_workflow("run: cargo binstall oakum@latest\n").unwrap_err();
        assert_eq!(err, PinError::Inexact(String::from("latest")));
        let err = versions_in_workflow("run: cargo binstall oakum@^1.0.0\n").unwrap_err();
        assert_eq!(err, PinError::Inexact(String::from("^1.0.0")));
    }

    /// A `--version` flag that carries a value names a spec, exact or not, so
    /// the install is versioned and the spec is what is wrong.
    #[test]
    fn an_inexact_version_flag_is_named_as_the_spec() {
        for (line, spec) in [
            ("run: cargo install oakum --version=latest\n", "latest"),
            ("run: cargo install oakum --version latest\n", "latest"),
            ("run: cargo install oakum --vers=^1\n", "^1"),
            ("run: cargo binstall oakum --version latest\n", "latest"),
            ("run: cargo install oakum --version=\n", ""),
        ] {
            assert_eq!(
                versions_in_workflow(line).unwrap_err(),
                PinError::Inexact(String::from(spec)),
                "{line}"
            );
        }
        assert_eq!(
            versions_in_workflow("run: cargo install oakum --version --locked\n").unwrap_err(),
            PinError::Unversioned
        );
    }

    #[test]
    fn npm_and_pnpm_install_lines_yield_the_version() {
        for line in [
            "run: pnpm add -D @oakoss/oakum@1.2.3\n",
            "run: pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@1.2.3\n",
            "run: pnpm add -Dw @oakoss/oakum@1.2.3\n",
            "run: npm i -g @oakoss/oakum@1.2.3\n",
            "run: npm install --global @oakoss/oakum@1.2.3\n",
            "run: npx @oakoss/oakum@1.2.3 check --strict\n",
            "run: pnpm dlx @oakoss/oakum@1.2.3 check\n",
            "run: pnpm install -g @oakoss/oakum@1.2.3\n",
            "run: sudo npm i -g @oakoss/oakum@1.2.3\n",
            "run: CI=1 npm i -g @oakoss/oakum@1.2.3 --save-exact\n",
            "run: (npm i -g \"@oakoss/oakum@1.2.3\")\n",
            "run: npx -y @oakoss/oakum@1.2.3 check\n",
            "run: npx --package=@oakoss/oakum@1.2.3 oakum check\n",
            "run: npx -p @oakoss/oakum@1.2.3 oakum check\n",
            "run: pnpm add -D @oakoss/oakum@1.2.3 @other/oakum@9.9.9\n",
            "run: pnpm -w add -D @oakoss/oakum@1.2.3\n",
            "run: pnpm --filter demo add -D @oakoss/oakum@1.2.3\n",
            "run: npm --prefix . i -g @oakoss/oakum@1.2.3\n",
            "run: npm -g i @oakoss/oakum@1.2.3\n",
            "run: sudo -E npm i -g @oakoss/oakum@1.2.3\n",
        ] {
            assert_eq!(
                versions_in_workflow(line).expect(line),
                vec![Version::parse("1.2.3").unwrap()],
                "{line}"
            );
        }
    }

    #[test]
    fn unversioned_npm_install_is_unverified() {
        for line in [
            "run: pnpm add -D @oakoss/oakum\n",
            "run: npm i -g @oakoss/oakum\n",
            "run: npx @oakoss/oakum check\n",
            "run: pnpm add -D @oakoss/oakum@1.2.3 && npm i -g @oakoss/oakum\n",
            "run: pnpm -w add -D @oakoss/oakum\n",
            "run: sudo -E npm i -g @oakoss/oakum\n",
        ] {
            assert_eq!(
                versions_in_workflow(line).unwrap_err(),
                PinError::Unversioned,
                "{line}"
            );
        }
    }

    #[test]
    fn npm_dist_tag_is_not_an_exact_pin() {
        let err = versions_in_workflow("run: pnpm add -D @oakoss/oakum@latest\n").unwrap_err();
        assert_eq!(err, PinError::Inexact(String::from("latest")));
    }

    #[test]
    fn npm_invocations_and_other_packages_are_not_pins() {
        for line in [
            "run: pnpm exec oakum check --strict\n",
            "run: pnpm install\n",
            "run: pnpm add -D @oakoss/other@1.2.3\n",
            "run: pnpm add -D @oakoss/oakum-extra@1.2.3\n",
            "run: npm run oakum\n",
            "run: npx some-tool @oakoss/oakum@1.2.3\n",
            "run: pnpm dlx some-tool @oakoss/oakum\n",
        ] {
            assert_eq!(
                versions_in_workflow(line).expect(line),
                Vec::new(),
                "{line}"
            );
        }
    }
}
