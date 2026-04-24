use std::collections::HashMap;

pub fn get_arg_value(args: &[String], name: &str) -> Option<String> {
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

pub fn parse_optional_int(value: Option<&str>) -> Option<i64> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<i64>().ok()
}

pub fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn flag_bool(
    args: &[String],
    env: &HashMap<String, String>,
    _name: &str,
    env_key: &str,
    cli_disable: &str,
    default: bool,
) -> bool {
    if args.iter().any(|arg| arg == cli_disable) {
        return false;
    }
    if let Some(v) = env.get(env_key) {
        return v != "0";
    }
    default
}

pub fn flag_bool_simple(
    args: &[String],
    env: &HashMap<String, String>,
    env_key: &str,
    cli_flag: &str,
    default: bool,
) -> bool {
    if args.iter().any(|arg| arg == cli_flag) {
        return true;
    }
    if let Some(v) = env.get(env_key) {
        return v != "0";
    }
    default
}

pub fn flag_opt<T: From<Vec<String>>>(
    args: &[String],
    env: &HashMap<String, String>,
    name: &str,
    env_key: &str,
    default: Option<T>,
) -> Option<T> {
    let mut values = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == &format!("--{name}") {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    values.extend(split_list(next));
                    i += 1;
                }
            }
        } else if let Some(rest) = arg.strip_prefix(&format!("--{name}=")) {
            values.extend(split_list(rest));
        }
        i += 1;
    }

    if !values.is_empty() {
        return Some(T::from(values));
    }

    if let Some(v) = env.get(env_key) {
        let env_values = split_list(v);
        if !env_values.is_empty() {
            return Some(T::from(env_values));
        }
    }

    default
}

pub fn flag_enum<T: Clone + Default>(
    args: &[String],
    env: &HashMap<String, String>,
    name: &str,
    env_key: &str,
    cli_name: Option<&str>,
    normalizer: fn(Option<&str>) -> Option<T>,
    default: T,
) -> T {
    let cli_name = cli_name.unwrap_or(name);

    if let Some(cli_val) = get_arg_value(args, cli_name) {
        if let Some(normalized) = normalizer(Some(&cli_val)) {
            return normalized;
        }
    }

    if let Some(v) = env.get(env_key) {
        if let Some(normalized) = normalizer(Some(v.as_str())) {
            return normalized;
        }
    }

    default
}

pub fn flag_usize(
    args: &[String],
    env: &HashMap<String, String>,
    name: &str,
    env_key: &str,
    default: usize,
) -> usize {
    let arg_value = get_arg_value(args, name);
    if let Some(v) = parse_optional_int(arg_value.as_deref()) {
        return v.max(0) as usize;
    }

    if let Some(v) = env.get(env_key) {
        if let Some(n) = parse_optional_int(Some(v)) {
            return n.max(0) as usize;
        }
    }

    default
}
