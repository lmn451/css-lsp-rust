use std::collections::HashMap;

use crate::flags::{
    flag_bool, flag_bool_simple, flag_enum, flag_opt, get_arg_value, parse_optional_int,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathDisplayMode {
    #[default]
    Relative,
    Absolute,
    Abbreviated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UndefinedVarFallbackMode {
    #[default]
    Warning,
    Info,
    Off,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub enable_color_provider: bool,
    pub color_only_on_variables: bool,
    pub lookup_files: Option<Vec<String>>,
    pub ignore_globs: Option<Vec<String>>,
    pub path_display_mode: PathDisplayMode,
    pub path_display_abbrev_length: usize,
    pub undefined_var_fallback: UndefinedVarFallbackMode,
    pub suggest_add_fallback: bool,
    pub suggest_exact_color_variables: bool,
}

fn normalize_path_display_mode(value: Option<&str>) -> Option<PathDisplayMode> {
    let raw = value?.trim().to_lowercase();
    match raw.as_str() {
        "relative" => Some(PathDisplayMode::Relative),
        "absolute" => Some(PathDisplayMode::Absolute),
        "abbreviated" | "abbr" | "fish" => Some(PathDisplayMode::Abbreviated),
        _ => None,
    }
}

fn normalize_undefined_var_fallback_mode(value: Option<&str>) -> Option<UndefinedVarFallbackMode> {
    let raw = value?.trim().to_lowercase();
    match raw.as_str() {
        "warning" | "warn" => Some(UndefinedVarFallbackMode::Warning),
        "info" | "information" => Some(UndefinedVarFallbackMode::Info),
        "off" | "omit" | "none" | "disable" | "disabled" => Some(UndefinedVarFallbackMode::Off),
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

pub fn build_runtime_config_with_env(
    args: &[String],
    env: &HashMap<String, String>,
) -> RuntimeConfig {
    let enable_color_provider = flag_bool(
        args,
        env,
        "color-preview",
        "CSS_LSP_COLOR_PREVIEW",
        "--no-color-preview",
        true,
    );

    let color_only_on_variables = flag_bool_simple(
        args,
        env,
        "CSS_LSP_COLOR_ONLY_VARIABLES",
        "--color-only-variables",
        false,
    );

    let lookup_files = flag_opt(args, env, "lookup-files", "CSS_LSP_LOOKUP_FILES", None);

    let ignore_globs = flag_opt(args, env, "ignore-globs", "CSS_LSP_IGNORE_GLOBS", None);

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

    let undefined_var_fallback = flag_enum(
        args,
        env,
        "undefined-var-fallback",
        "CSS_LSP_UNDEFINED_VAR_FALLBACK",
        None,
        normalize_undefined_var_fallback_mode,
        UndefinedVarFallbackMode::Warning,
    );

    let suggest_add_fallback = flag_bool(
        args,
        env,
        "suggest-add-fallback",
        "CSS_LSP_SUGGEST_ADD_FALLBACK",
        "--no-suggest-add-fallback",
        true,
    );

    let suggest_exact_color_variables = flag_bool(
        args,
        env,
        "suggest-exact-color-variables",
        "CSS_LSP_SUGGEST_EXACT_COLOR_VARIABLES",
        "--no-suggest-exact-color-variables",
        true,
    );

    RuntimeConfig {
        enable_color_provider,
        color_only_on_variables,
        lookup_files,
        ignore_globs,
        path_display_mode,
        path_display_abbrev_length,
        undefined_var_fallback,
        suggest_add_fallback,
        suggest_exact_color_variables,
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
            "--ignore-globs=dist/**".to_string(),
            "--path-display=abbreviated:2".to_string(),
            "--undefined-var-fallback=info".to_string(),
        ];
        let mut env = HashMap::new();
        env.insert(
            "CSS_LSP_LOOKUP_FILES".to_string(),
            "ignored.css".to_string(),
        );
        env.insert("CSS_LSP_IGNORE_GLOBS".to_string(), "ignored/**".to_string());
        env.insert("CSS_LSP_PATH_DISPLAY".to_string(), "absolute".to_string());
        env.insert(
            "CSS_LSP_UNDEFINED_VAR_FALLBACK".to_string(),
            "off".to_string(),
        );

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
        assert_eq!(
            config.undefined_var_fallback,
            UndefinedVarFallbackMode::Info
        );
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
        env.insert(
            "CSS_LSP_UNDEFINED_VAR_FALLBACK".to_string(),
            "omit".to_string(),
        );

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
        assert_eq!(config.undefined_var_fallback, UndefinedVarFallbackMode::Off);
    }

    #[test]
    fn runtime_config_undefined_var_fallback_defaults_on_invalid() {
        let args = vec!["--undefined-var-fallback=maybe".to_string()];
        let mut env = HashMap::new();
        env.insert(
            "CSS_LSP_UNDEFINED_VAR_FALLBACK".to_string(),
            "surely".to_string(),
        );

        let config = build_runtime_config_with_env(&args, &env);

        assert_eq!(
            config.undefined_var_fallback,
            UndefinedVarFallbackMode::Warning
        );
    }

    #[test]
    fn runtime_config_suggest_add_fallback_enabled_by_default() {
        let args: Vec<String> = Vec::new();
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);
        assert!(config.suggest_add_fallback);
    }

    #[test]
    fn runtime_config_suggest_add_fallback_disabled_by_flag() {
        let args = vec!["--no-suggest-add-fallback".to_string()];
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);
        assert!(!config.suggest_add_fallback);
    }

    #[test]
    fn runtime_config_suggest_add_fallback_disabled_by_env() {
        let args: Vec<String> = Vec::new();
        let mut env = HashMap::new();
        env.insert("CSS_LSP_SUGGEST_ADD_FALLBACK".to_string(), "0".to_string());
        let config = build_runtime_config_with_env(&args, &env);
        assert!(!config.suggest_add_fallback);
    }

    #[test]
    fn runtime_config_suggest_add_fallback_enabled_by_env() {
        let args: Vec<String> = Vec::new();
        let mut env = HashMap::new();
        env.insert("CSS_LSP_SUGGEST_ADD_FALLBACK".to_string(), "1".to_string());
        let config = build_runtime_config_with_env(&args, &env);
        assert!(config.suggest_add_fallback);
    }

    #[test]
    fn runtime_config_suggest_exact_color_variables_enabled_by_default() {
        let args: Vec<String> = Vec::new();
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);
        assert!(config.suggest_exact_color_variables);
    }

    #[test]
    fn runtime_config_suggest_exact_color_variables_disabled_by_flag() {
        let args = vec!["--no-suggest-exact-color-variables".to_string()];
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);
        assert!(!config.suggest_exact_color_variables);
    }

    #[test]
    fn runtime_config_suggest_exact_color_variables_disabled_by_env() {
        let args: Vec<String> = Vec::new();
        let mut env = HashMap::new();
        env.insert(
            "CSS_LSP_SUGGEST_EXACT_COLOR_VARIABLES".to_string(),
            "0".to_string(),
        );
        let config = build_runtime_config_with_env(&args, &env);
        assert!(!config.suggest_exact_color_variables);
    }

    #[test]
    fn runtime_config_accepts_singular_lookup_file_flag() {
        let args = vec![
            "--lookup-file".to_string(),
            "a.css".to_string(),
            "--lookup-file".to_string(),
            "b.scss".to_string(),
        ];
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);

        assert_eq!(
            config.lookup_files.as_ref().unwrap(),
            &vec!["a.css".to_string(), "b.scss".to_string()]
        );
    }

    #[test]
    fn runtime_config_accepts_singular_ignore_glob_flag() {
        let args = vec![
            "--ignore-glob".to_string(),
            "dist/**".to_string(),
            "--ignore-glob".to_string(),
            "node_modules/**".to_string(),
        ];
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);

        assert_eq!(
            config.ignore_globs.as_ref().unwrap(),
            &vec!["dist/**".to_string(), "node_modules/**".to_string()]
        );
    }

    #[test]
    fn runtime_config_primary_and_singular_flags_work_together() {
        let args = vec![
            "--lookup-files".to_string(),
            "a.css".to_string(),
            "--lookup-file".to_string(),
            "b.scss".to_string(),
            "--ignore-globs".to_string(),
            "dist/**".to_string(),
            "--ignore-glob".to_string(),
            "out/**".to_string(),
        ];
        let env = HashMap::new();
        let config = build_runtime_config_with_env(&args, &env);

        assert_eq!(
            config.lookup_files.as_ref().unwrap(),
            &vec!["a.css".to_string(), "b.scss".to_string()]
        );
        assert_eq!(
            config.ignore_globs.as_ref().unwrap(),
            &vec!["dist/**".to_string(), "out/**".to_string()]
        );
    }
}
