use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDisplayMode {
    Relative,
    Absolute,
    Abbreviated,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub enable_color_provider: bool,
    pub color_only_on_variables: bool,
    pub lookup_files: Option<Vec<String>>,
    pub ignore_globs: Option<Vec<String>>,
    pub path_display_mode: PathDisplayMode,
    pub path_display_abbrev_length: usize,
}

fn get_arg_value(args: &[String], name: &str) -> Option<String> {
    let flag = format!("--{name}");
    if let Some(idx) = args.iter().position(|arg| arg == &flag) {
        if let Some(candidate) = args.get(idx + 1) {
            if !candidate.starts_with('-') {
                return Some(candidate.to_string());
            }
        }
        return None;
    }

    let prefix = format!("{}=", flag);
    for arg in args {
        if arg.starts_with(&prefix) {
            return Some(arg[prefix.len()..].to_string());
        }
    }

    None
}

fn parse_optional_int(value: Option<&str>) -> Option<i64> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<i64>().ok()
}

fn normalize_path_display_mode(value: Option<&str>) -> Option<PathDisplayMode> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.to_lowercase().as_str() {
        "relative" => Some(PathDisplayMode::Relative),
        "absolute" => Some(PathDisplayMode::Absolute),
        "abbreviated" | "abbr" | "fish" => Some(PathDisplayMode::Abbreviated),
        _ => None,
    }
}

fn parse_path_display(value: Option<&str>) -> (Option<PathDisplayMode>, Option<i64>) {
    let raw = match value {
        Some(v) if !v.trim().is_empty() => v,
        _ => return (None, None),
    };

    let mut parts = raw.splitn(2, ':');
    let mode_part = parts.next();
    let length_part = parts.next();

    (
        normalize_path_display_mode(mode_part),
        parse_optional_int(length_part),
    )
}

fn split_lookup_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

fn resolve_lookup_files(args: &[String], env: &HashMap<String, String>) -> Option<Vec<String>> {
    let mut cli_files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--lookup-files" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    cli_files.extend(split_lookup_list(next));
                    i += 1;
                }
            }
        } else if let Some(rest) = arg.strip_prefix("--lookup-files=") {
            cli_files.extend(split_lookup_list(rest));
        } else if arg == "--lookup-file" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    cli_files.push(next.to_string());
                    i += 1;
                }
            }
        } else if let Some(rest) = arg.strip_prefix("--lookup-file=") {
            cli_files.push(rest.to_string());
        }
        i += 1;
    }

    if !cli_files.is_empty() {
        return Some(cli_files);
    }

    if let Some(env_value) = env.get("CSS_LSP_LOOKUP_FILES") {
        let env_files = split_lookup_list(env_value);
        if !env_files.is_empty() {
            return Some(env_files);
        }
    }

    None
}

fn resolve_ignore_globs(args: &[String], env: &HashMap<String, String>) -> Option<Vec<String>> {
    let mut cli_globs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--ignore-globs" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    cli_globs.extend(split_lookup_list(next));
                    i += 1;
                }
            }
        } else if let Some(rest) = arg.strip_prefix("--ignore-globs=") {
            cli_globs.extend(split_lookup_list(rest));
        } else if arg == "--ignore-glob" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    cli_globs.push(next.to_string());
                    i += 1;
                }
            }
        } else if let Some(rest) = arg.strip_prefix("--ignore-glob=") {
            cli_globs.push(rest.to_string());
        }
        i += 1;
    }

    if !cli_globs.is_empty() {
        return Some(cli_globs);
    }

    if let Some(env_value) = env.get("CSS_LSP_IGNORE_GLOBS") {
        let env_globs = split_lookup_list(env_value);
        if !env_globs.is_empty() {
            return Some(env_globs);
        }
    }

    None
}

pub fn build_runtime_config_with_env(
    args: &[String],
    env: &HashMap<String, String>,
) -> RuntimeConfig {
    let enable_color_provider = !args.iter().any(|arg| arg == "--no-color-preview");
    let color_only_on_variables = args.iter().any(|arg| arg == "--color-only-variables")
        || env
            .get("CSS_LSP_COLOR_ONLY_VARIABLES")
            .map(|v| v == "1")
            .unwrap_or(false);

    let lookup_files = resolve_lookup_files(args, env);
    let ignore_globs = resolve_ignore_globs(args, env);

    let path_display_arg = get_arg_value(args, "path-display");
    let path_display_env = env.get("CSS_LSP_PATH_DISPLAY").cloned();
    let (mode_override, length_override) =
        parse_path_display(path_display_arg.as_deref().or(path_display_env.as_deref()));

    let path_display_mode = mode_override.unwrap_or(PathDisplayMode::Relative);

    let length_arg = get_arg_value(args, "path-display-length");
    let length_env = env.get("CSS_LSP_PATH_DISPLAY_LENGTH").cloned();
    let length_raw = parse_optional_int(length_arg.as_deref().or(length_env.as_deref()))
        .or(length_override)
        .unwrap_or(1);
    let path_display_abbrev_length = length_raw.max(0) as usize;

    RuntimeConfig {
        enable_color_provider,
        color_only_on_variables,
        lookup_files,
        ignore_globs,
        path_display_mode,
        path_display_abbrev_length,
    }
}

pub fn build_runtime_config(args: &[String]) -> RuntimeConfig {
    let env: HashMap<String, String> = std::env::vars().collect();
    build_runtime_config_with_env(args, &env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_config_prefers_cli_over_env() {
        let args = vec![
            "--no-color-preview".to_string(),
            "--color-only-variables".to_string(),
            "--lookup-files".to_string(),
            "a.css,b.html".to_string(),
            "--ignore-glob=dist/**".to_string(),
            "--path-display=abbreviated:2".to_string(),
        ];
        let mut env = HashMap::new();
        env.insert(
            "CSS_LSP_LOOKUP_FILES".to_string(),
            "ignored.css".to_string(),
        );
        env.insert("CSS_LSP_IGNORE_GLOBS".to_string(), "ignored/**".to_string());
        env.insert("CSS_LSP_PATH_DISPLAY".to_string(), "absolute".to_string());

        let config = build_runtime_config_with_env(&args, &env);

        assert!(!config.enable_color_provider);
        assert!(config.color_only_on_variables);
        assert_eq!(
            config.lookup_files.as_ref().unwrap(),
            &vec!["a.css".to_string(), "b.html".to_string()]
        );
        assert_eq!(
            config.ignore_globs.as_ref().unwrap(),
            &vec!["dist/**".to_string()]
        );
        assert_eq!(config.path_display_mode, PathDisplayMode::Abbreviated);
        assert_eq!(config.path_display_abbrev_length, 2);
    }

    #[test]
    fn runtime_config_uses_env_when_cli_missing() {
        let args: Vec<String> = Vec::new();
        let mut env = HashMap::new();
        env.insert(
            "CSS_LSP_LOOKUP_FILES".to_string(),
            "one.css,two.html".to_string(),
        );
        env.insert(
            "CSS_LSP_IGNORE_GLOBS".to_string(),
            "dist/**,out/**".to_string(),
        );
        env.insert("CSS_LSP_PATH_DISPLAY".to_string(), "relative".to_string());
        env.insert("CSS_LSP_PATH_DISPLAY_LENGTH".to_string(), "3".to_string());

        let config = build_runtime_config_with_env(&args, &env);

        assert!(config.enable_color_provider);
        assert!(!config.color_only_on_variables);
        assert_eq!(
            config.lookup_files.as_ref().unwrap(),
            &vec!["one.css".to_string(), "two.html".to_string()]
        );
        assert_eq!(
            config.ignore_globs.as_ref().unwrap(),
            &vec!["dist/**".to_string(), "out/**".to_string()]
        );
        assert_eq!(config.path_display_mode, PathDisplayMode::Relative);
        assert_eq!(config.path_display_abbrev_length, 3);
    }
}
