use std::collections::HashMap;

use crate::types::Config;

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfigPatch {
    pub lookup_files: Option<Vec<String>>,
    pub ignore_globs: Option<Vec<String>>,
    pub enable_color_provider: Option<bool>,
    pub color_only_on_variables: Option<bool>,
}

pub fn apply_config_patch(mut base: Config, patch: ClientConfigPatch) -> Config {
    if let Some(lookup_files) = patch.lookup_files {
        base.lookup_files = lookup_files;
    }
    if let Some(ignore_globs) = patch.ignore_globs {
        base.ignore_globs = ignore_globs;
    }
    if let Some(enable_color_provider) = patch.enable_color_provider {
        base.enable_color_provider = enable_color_provider;
    }
    if let Some(color_only_on_variables) = patch.color_only_on_variables {
        base.color_only_on_variables = color_only_on_variables;
    }
    base
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Css,
    Html,
}

fn is_html_like_extension(ext: &str) -> bool {
    matches!(ext, ".html" | ".vue" | ".svelte" | ".astro" | ".ripple")
}

pub fn language_id_kind(language_id: &str) -> Option<DocumentKind> {
    match language_id.to_lowercase().as_str() {
        "html" | "vue" | "svelte" | "astro" | "ripple" => Some(DocumentKind::Html),
        "css" | "scss" | "sass" | "less" => Some(DocumentKind::Css),
        _ => None,
    }
}

pub(crate) fn normalize_extension(ext: &str) -> Option<String> {
    let trimmed = ext.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(".{}", trimmed.to_lowercase()))
}

fn extract_extensions(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    if let (Some(start), Some(end)) = (pattern.find('{'), pattern.find('}')) {
        if end > start + 1 {
            let inner = &pattern[start + 1..end];
            return inner.split(',').filter_map(normalize_extension).collect();
        }
    }

    let ext = std::path::Path::new(pattern)
        .extension()
        .and_then(|ext| ext.to_str());
    ext.and_then(normalize_extension).into_iter().collect()
}

pub fn build_lookup_extension_map(lookup_files: &[String]) -> HashMap<String, DocumentKind> {
    let mut map = HashMap::new();
    for pattern in lookup_files {
        for ext in extract_extensions(pattern) {
            let kind = if is_html_like_extension(&ext) {
                DocumentKind::Html
            } else {
                DocumentKind::Css
            };
            map.insert(ext, kind);
        }
    }
    map
}

pub fn resolve_document_kind(
    path: &str,
    language_id: Option<&str>,
    lookup_extension_map: &HashMap<String, DocumentKind>,
) -> Option<DocumentKind> {
    if let Some(language_id) = language_id {
        if let Some(kind) = language_id_kind(language_id) {
            return Some(kind);
        }
    }

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(normalize_extension)?;

    lookup_extension_map.get(&ext).copied()
}
