use globset::{Glob, GlobSetBuilder};
use std::fs;
use std::path::PathBuf;
use tower_lsp::lsp_types::Url;
use walkdir::WalkDir;

use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document};

/// Statistics collected during workspace scanning
#[derive(Debug, Default)]
pub struct ScanStats {
    /// Total files matched by glob patterns
    pub files_matched: usize,
    /// Files successfully parsed
    pub files_parsed: usize,
    /// Files that failed to read (permission denied, encoding errors, etc.)
    pub read_errors: usize,
    /// Files that failed to parse
    pub parse_errors: usize,
    /// Sample error messages (up to 5) for debugging
    pub error_samples: Vec<String>,
}

impl ScanStats {
    pub fn add_error(&mut self, msg: String) {
        if self.error_samples.len() < 5 {
            self.error_samples.push(msg);
        }
    }

    /// Returns a human-readable summary of the scan
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} files scanned", self.files_matched)];

        if self.read_errors > 0 {
            parts.push(format!("{} read errors", self.read_errors));
        }
        if self.parse_errors > 0 {
            parts.push(format!("{} parse errors", self.parse_errors));
        }

        parts.join(", ")
    }

    /// Returns detailed error information if any errors occurred
    pub fn error_details(&self) -> Option<String> {
        if self.error_samples.is_empty() {
            return None;
        }

        let mut details = String::from("Error samples:\n");
        for sample in &self.error_samples {
            details.push_str(&format!("  - {}\n", sample));
        }

        let total_errors = self.read_errors + self.parse_errors;
        if total_errors > self.error_samples.len() {
            details.push_str(&format!(
                "  ... and {} more errors\n",
                total_errors - self.error_samples.len()
            ));
        }

        Some(details)
    }
}

/// Scan workspace folders for CSS and HTML files
///
/// Returns scan statistics including any errors encountered.
/// Note: SCSS/SASS/LESS files are parsed for CSS custom properties (--var) only.
/// Native preprocessor variables ($var in SCSS) are not supported.
pub async fn scan_workspace(
    folders: Vec<Url>,
    manager: &CssVariableManager,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<ScanStats, String> {
    let config = manager.get_config().await;
    let mut stats = ScanStats::default();

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
    stats.files_matched = total;

    // Parse each file
    for (i, file_path) in all_files.iter().enumerate() {
        // Report progress
        on_progress(i + 1, total);

        // Read file content
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                stats.read_errors += 1;
                stats.add_error(format!("{}: {}", file_path.display(), e));
                continue;
            }
        };

        // Convert to URI
        let file_uri = match Url::from_file_path(file_path) {
            Ok(u) => u,
            Err(_) => {
                stats.read_errors += 1;
                stats.add_error(format!("{}: invalid path for URI", file_path.display()));
                continue;
            }
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

        match result {
            Ok(()) => {
                stats.files_parsed += 1;
            }
            Err(e) => {
                stats.parse_errors += 1;
                stats.add_error(format!("{}: {}", file_path.display(), e));
            }
        }
    }

    Ok(stats)
}
