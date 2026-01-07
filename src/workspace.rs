use globset::{Glob, GlobSetBuilder};
use std::fs;
use std::path::PathBuf;
use tower_lsp::lsp_types::Url;
use walkdir::WalkDir;

use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document};

/// Scan workspace folders for CSS and HTML files
pub async fn scan_workspace(
    folders: Vec<Url>,
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
    let mut all_files = Vec::new();

    for folder_uri in folders {
        let folder_path = PathBuf::from(folder_uri.path());

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
                all_files.push(path.to_path_buf());
            }
        }
    }

    let total = all_files.len();

    // Parse each file
    for (i, file_path) in all_files.iter().enumerate() {
        // Report progress
        on_progress(i + 1, total);

        // Read file content
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Convert to URI
        let file_uri = match Url::from_file_path(file_path) {
            Ok(u) => u,
            Err(_) => continue,
        };

        // Determine file type and parse
        let path_str = file_path.to_string_lossy();
        let result = if path_str.ends_with(".html")
            || path_str.ends_with(".vue")
            || path_str.ends_with(".svelte")
            || path_str.ends_with(".astro")
            || path_str.ends_with(".ripple")
        {
            parse_html_document(&content, &file_uri, manager).await
        } else if path_str.ends_with(".css")
            || path_str.ends_with(".scss")
            || path_str.ends_with(".sass")
            || path_str.ends_with(".less")
        {
            parse_css_document(&content, &file_uri, manager).await
        } else {
            continue;
        };

        // Log errors but continue
        if let Err(_e) = result {
            // Silent error - could log if needed
        }
    }

    Ok(())
}
