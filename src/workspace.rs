use globset::{Glob, GlobSetBuilder};
use ls_types::Uri;
use std::collections::HashSet;
use tokio::fs;
use walkdir::WalkDir;

use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document};

/// Scan workspace folders for CSS and HTML files.
///
/// Uses the configured `lookup_files` glob patterns to discover files and the
/// `ignore_globs` patterns to exclude them. Document kind is resolved via
/// `document_kind::resolve_document_kind` so that workspace scanning stays in
/// sync with completion / hover / goto-definition behavior.
pub async fn scan_workspace(
    folders: Vec<Uri>,
    manager: &CssVariableManager,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(), String> {
    let config = manager.get_config().await;

    // Build glob matchers for lookup patterns
    let mut lookup_builder = GlobSetBuilder::new();
    for pattern in &config.lookup_files {
        if let Ok(glob) = Glob::new(pattern) {
            lookup_builder.add(glob);
        }
    }
    let lookup_set = lookup_builder
        .build()
        .map_err(|e| format!("Failed to build lookup glob set: {}", e))?;

    // Build glob matchers for ignore patterns
    let mut ignore_builder = GlobSetBuilder::new();
    for pattern in &config.ignore_globs {
        if let Ok(glob) = Glob::new(pattern) {
            ignore_builder.add(glob);
        }
    }
    let ignore_set = ignore_builder
        .build()
        .map_err(|e| format!("Failed to build ignore glob set: {}", e))?;

    // Collect all files from all folders
    let mut all_files = HashSet::new();

    for folder_uri in folders {
        let folder_path = match crate::path_display::to_normalized_fs_path(&folder_uri) {
            Some(path) => path,
            None => continue,
        };

        for entry in WalkDir::new(&folder_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Skip if not a file
            if !path.is_file() {
                continue;
            }

            // Get relative path for glob matching
            let relative = match path.strip_prefix(&folder_path) {
                Ok(rel) => rel,
                Err(_) => continue,
            };

            // Convert to string for glob matching
            let path_str = relative.to_string_lossy();

            // Skip if matches ignore pattern
            if ignore_set.is_match(&*path_str) {
                continue;
            }

            // Include if matches lookup pattern
            if lookup_set.is_match(&*path_str) {
                all_files.insert(path.to_path_buf());
            }
        }
    }

    let mut all_files: Vec<_> = all_files.into_iter().collect();
    all_files.sort();
    let total = all_files.len();

    // A scan replaces the previously indexed state for every discovered file. This
    // keeps repeated scans and overlapping workspace folders from accumulating copies.
    let file_uris: HashSet<Uri> = all_files.iter().filter_map(Uri::from_file_path).collect();
    manager.remove_documents(&file_uris).await;

    // Build the extension->kind map once for the whole scan so we agree with the
    // editor on what counts as CSS vs HTML vs JS without re-deriving it per file.
    let lookup_map = crate::document_kind::build_lookup_extension_map(&config.lookup_files);

    // Parse each file
    for (i, file_path) in all_files.iter().enumerate() {
        // Report progress
        on_progress(i + 1, total);

        // Read file content
        let content = match fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Convert to URI
        let file_uri = match Uri::from_file_path(file_path) {
            Some(u) => u,
            None => continue,
        };

        // Determine file type and parse using the extension->kind map built once above.
        let path_str = file_path.to_string_lossy();
        let kind = match crate::document_kind::resolve_document_kind(&path_str, None, &lookup_map) {
            Some(kind) => kind,
            None => continue,
        };
        let result = match kind {
            crate::document_kind::DocumentKind::Html => {
                parse_html_document(&content, &file_uri, manager).await
            }
            crate::document_kind::DocumentKind::Css => {
                parse_css_document(&content, &file_uri, manager).await
            }
            // JS/CSS-in-JS files are not scanned eagerly from disk: their CSS lives
            // inside string/template literals that we only parse when the editor is
            // actually showing us the file (did_open/did_change).
            crate::document_kind::DocumentKind::Js => continue,
        };

        // Log errors but continue so a single malformed file does not abort the
        // whole workspace scan. Only logged when the user has opted into tracing
        // via CSS_LSP_ENABLE_LOGS=1 so we keep LSP stdio clean by default.
        if let Err(e) = result {
            tracing::debug!(file = %file_path.display(), error = %e, "workspace scan: parse error");
        }
    }

    manager.rebuild_color_index().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Config;

    #[tokio::test]
    async fn repeated_scans_replace_existing_document_state() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-repeat-scan-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("variables.css"), ":root { --primary: red; }").unwrap();

        let manager = CssVariableManager::new(Config::default());
        let root_uri = Uri::from_file_path(&root).unwrap();
        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();

        assert_eq!(manager.get_variables("--primary").await.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
