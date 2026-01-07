use std::path::{Component, Path, PathBuf};
use tower_lsp::lsp_types::Url;

use crate::runtime_config::PathDisplayMode;

pub struct PathDisplayOptions<'a> {
    pub mode: PathDisplayMode,
    pub abbrev_length: usize,
    pub workspace_folder_paths: &'a [PathBuf],
    pub root_folder_path: Option<&'a PathBuf>,
}

pub fn to_normalized_fs_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok()
}

fn find_best_relative_path(fs_path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    for root in roots {
        let relative = match pathdiff::diff_paths(fs_path, root) {
            Some(rel) => rel,
            None => continue,
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        if relative.is_absolute() {
            continue;
        }
        if matches!(relative.components().next(), Some(Component::ParentDir)) {
            continue;
        }
        let rel_len = relative.to_string_lossy().len();
        let best_len = best
            .as_ref()
            .map(|p| p.to_string_lossy().len())
            .unwrap_or(usize::MAX);
        if rel_len < best_len {
            best = Some(relative);
        }
    }
    best
}

fn abbreviate_path(path: &Path, abbrev_length: usize) -> String {
    let path_str = path.to_string_lossy();
    if abbrev_length == 0 {
        return path_str.to_string();
    }
    let mut parts: Vec<String> = path
        .components()
        .filter_map(|comp| match comp {
            Component::Normal(p) => Some(p.to_string_lossy().to_string()),
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().to_string()),
            Component::RootDir => Some(std::path::MAIN_SEPARATOR.to_string()),
            _ => None,
        })
        .collect();

    if parts.len() <= 1 {
        return path_str.to_string();
    }

    let last_index = parts.len() - 1;
    for (idx, part) in parts.iter_mut().enumerate() {
        if idx == last_index {
            continue;
        }
        if part.len() > abbrev_length {
            *part = part[..abbrev_length].to_string();
        }
    }

    if parts
        .first()
        .map(|p| p == &std::path::MAIN_SEPARATOR.to_string())
        .unwrap_or(false)
    {
        let mut rebuilt = String::new();
        rebuilt.push(std::path::MAIN_SEPARATOR);
        rebuilt.push_str(&parts[1..].join(std::path::MAIN_SEPARATOR_STR));
        return rebuilt;
    }

    parts.join(std::path::MAIN_SEPARATOR_STR)
}

pub fn format_uri_for_display(uri: &Url, options: PathDisplayOptions<'_>) -> String {
    let fs_path = match to_normalized_fs_path(uri) {
        Some(path) => path,
        None => return uri.to_string(),
    };

    let roots: Vec<PathBuf> = if !options.workspace_folder_paths.is_empty() {
        options.workspace_folder_paths.to_vec()
    } else if let Some(root) = options.root_folder_path {
        vec![root.clone()]
    } else {
        Vec::new()
    };

    let relative = if roots.is_empty() {
        None
    } else {
        find_best_relative_path(&fs_path, &roots)
    };

    match options.mode {
        PathDisplayMode::Absolute => fs_path.to_string_lossy().to_string(),
        PathDisplayMode::Abbreviated => {
            let base = relative.as_ref().unwrap_or(&fs_path);
            abbreviate_path(base, options.abbrev_length)
        }
        PathDisplayMode::Relative => relative.unwrap_or(fs_path).to_string_lossy().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::MAIN_SEPARATOR;

    #[test]
    fn format_uri_respects_relative_and_abbreviated_modes() {
        let root = std::env::temp_dir().join("css-lsp-test-root");
        let file_path = root.join("src").join("styles").join("main.css");
        let uri = Url::from_file_path(&file_path).unwrap();

        let workspace_paths = vec![root.clone()];
        let relative = format_uri_for_display(
            &uri,
            PathDisplayOptions {
                mode: PathDisplayMode::Relative,
                abbrev_length: 1,
                workspace_folder_paths: &workspace_paths,
                root_folder_path: None,
            },
        );

        let expected_relative = format!("src{sep}styles{sep}main.css", sep = MAIN_SEPARATOR);
        assert_eq!(relative, expected_relative);

        let abbreviated = format_uri_for_display(
            &uri,
            PathDisplayOptions {
                mode: PathDisplayMode::Abbreviated,
                abbrev_length: 1,
                workspace_folder_paths: &workspace_paths,
                root_folder_path: None,
            },
        );

        let expected_abbrev = format!("s{sep}s{sep}main.css", sep = MAIN_SEPARATOR);
        assert_eq!(abbreviated, expected_abbrev);
    }
}
