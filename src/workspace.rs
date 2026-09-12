use globset::{Glob, GlobSetBuilder};
use ls_types::Uri;
use std::collections::HashSet;
use tokio::fs;
use walkdir::WalkDir;

use crate::config_analysis::{is_supported_config_path, parse_config_document, MAX_CONFIG_BYTES};
use crate::document_kind::{is_js_like_extension, normalize_extension};
use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document, parse_js_document};

/// Scan workspace folders for CSS, HTML, and optionally JavaScript files.
///
/// Uses the configured `lookup_files` glob patterns to discover files and the
/// `ignore_globs` patterns to exclude them. When `eager_js` is enabled, all
/// supported JavaScript/TypeScript extensions are also discovered for CSS-in-JS
/// extraction. Document kind is resolved via `document_kind::resolve_document_kind`
/// so that workspace scanning stays in sync with completion / hover /
/// goto-definition behavior.
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
    let mut scanned_folder_paths = Vec::new();

    for folder_uri in folders {
        let folder_path = match crate::path_display::to_normalized_fs_path(&folder_uri) {
            Some(path) => path,
            None => continue,
        };
        scanned_folder_paths.push(folder_path.clone());

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

            // Framework configuration sources are discovered by exact basename regardless
            // of lookup patterns; eager mode additionally selects ordinary JS/TS files.
            let is_eager_js = config.eager_js
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(normalize_extension)
                    .is_some_and(|extension| is_js_like_extension(&extension));
            if lookup_set.is_match(&*path_str) || is_supported_config_path(relative) || is_eager_js
            {
                all_files.insert(path.to_path_buf());
            }
        }
    }

    let mut all_files: Vec<_> = all_files.into_iter().collect();
    all_files.sort();
    let total = all_files.len();

    let discovered_uris: HashSet<Uri> = all_files.iter().filter_map(Uri::from_file_path).collect();
    let stale_uris: HashSet<Uri> = manager
        .get_document_uris()
        .await
        .into_iter()
        .filter(|uri| {
            let Some(path) = crate::path_display::to_normalized_fs_path(uri) else {
                return false;
            };
            scanned_folder_paths
                .iter()
                .any(|folder| path.strip_prefix(folder).is_ok())
                && !discovered_uris.contains(uri)
        })
        .collect();
    // This also removes stale JS/CSS-in-JS state when eager mode is disabled or
    // when a lookup/ignore pattern no longer selects a previously indexed file.
    manager.remove_documents(&stale_uris).await;

    // Normal discovered files are cleared as a batch. Configuration sources replace their
    // definitions atomically after successful analysis, preserving the last valid state while
    // an editor or disk file is temporarily malformed.
    let file_uris: HashSet<Uri> = all_files
        .iter()
        .filter(|path| !is_supported_config_path(path))
        .filter_map(Uri::from_file_path)
        .collect();
    manager.remove_documents(&file_uris).await;

    // Build the extension->kind map once for the whole scan so we agree with the
    // editor on what counts as CSS vs HTML vs JS without re-deriving it per file.
    let lookup_map = crate::document_kind::build_lookup_extension_map(&config.lookup_files);

    // Parse each file
    for (i, file_path) in all_files.iter().enumerate() {
        // Report progress
        on_progress(i + 1, total);

        let is_config = is_supported_config_path(file_path);
        if is_config {
            let oversized = fs::metadata(file_path)
                .await
                .is_ok_and(|metadata| metadata.len() > MAX_CONFIG_BYTES as u64);
            if oversized {
                tracing::debug!(
                    file = %file_path.display(),
                    limit = MAX_CONFIG_BYTES,
                    "workspace scan: skipped oversized configuration source"
                );
                continue;
            }
        }

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

        if is_config {
            if let Err(e) = parse_config_document(&content, &file_uri, manager).await {
                tracing::debug!(
                    file = %file_path.display(),
                    error = %e,
                    "workspace scan: configuration analysis error"
                );
            }
            continue;
        }

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
            crate::document_kind::DocumentKind::Js if config.eager_js => {
                parse_js_document(&content, &file_uri, manager).await
            }
            // Without eager mode, CSS-in-JS is parsed only while the editor
            // is showing the JavaScript/TypeScript document.
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

    #[tokio::test]
    async fn rescans_do_not_remove_documents_from_sibling_workspace_paths() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-sibling-workspace-scan-{}",
            std::process::id()
        ));
        let workspace = root.join("app");
        let sibling = root.join("app-old");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("variables.css"), ":root { --sibling: red; }").unwrap();

        let manager = CssVariableManager::new(Config::default());
        scan_workspace(
            vec![Uri::from_file_path(&sibling).unwrap()],
            &manager,
            |_, _| {},
        )
        .await
        .unwrap();
        scan_workspace(
            vec![Uri::from_file_path(&workspace).unwrap()],
            &manager,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--sibling").await.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rescans_remove_deleted_and_newly_ignored_astro_configs() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-config-rescan-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("astro.config.ts");
        let config_text = r#"export default { fonts: [{ cssVariable: "--font-scan" }] };"#;
        std::fs::write(&config_path, config_text).unwrap();

        let manager = CssVariableManager::new(Config::default());
        let root_uri = Uri::from_file_path(&root).unwrap();
        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        assert_eq!(manager.get_variables("--font-scan").await.len(), 1);

        std::fs::remove_file(&config_path).unwrap();
        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        assert!(manager.get_variables("--font-scan").await.is_empty());

        std::fs::write(&config_path, config_text).unwrap();
        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        assert_eq!(manager.get_variables("--font-scan").await.len(), 1);

        let mut config = manager.get_config().await;
        config.ignore_globs.push("astro.config.ts".to_string());
        manager.set_config(config).await;
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();
        assert!(manager.get_variables("--font-scan").await.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rescans_remove_deleted_vite_configs() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-vite-config-rescan-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("vite.config.ts");
        std::fs::write(
            &config_path,
            r#"
                export default {
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --vite-scan: red; }",
                            },
                        },
                    },
                };
            "#,
        )
        .unwrap();

        let manager = CssVariableManager::new(Config::default());
        let root_uri = Uri::from_file_path(&root).unwrap();
        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        assert_eq!(manager.get_variables("--vite-scan").await.len(), 1);

        std::fs::remove_file(&config_path).unwrap();
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();
        assert!(manager.get_variables("--vite-scan").await.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rescans_remove_deleted_usage_only_vite_configs() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-vite-usage-rescan-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("vite.config.ts");
        std::fs::write(
            &config_path,
            r#"export default { css: { preprocessorOptions: { scss: { additionalData: ":root { color: var(--external); }" } } } };"#,
        )
        .unwrap();
        let root_uri = Uri::from_file_path(&root).unwrap();
        let manager = CssVariableManager::new(Config::default());

        scan_workspace(vec![root_uri.clone()], &manager, |_, _| {})
            .await
            .unwrap();
        assert_eq!(manager.get_usages("--external").await.len(), 1);

        std::fs::remove_file(config_path).unwrap();
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();
        assert!(manager.get_usages("--external").await.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discovers_all_astro_config_extensions_and_skips_ignored_dirs() {
        let root = std::env::temp_dir().join(format!(
            "css-variable-lsp-astro-config-extensions-{}",
            std::process::id()
        ));
        let ignored = root.join("node_modules");
        std::fs::create_dir_all(&ignored).unwrap();

        let extensions = ["js", "mjs", "cjs", "ts", "mts", "cts"];
        for (index, extension) in extensions.iter().enumerate() {
            std::fs::write(
                root.join(format!("astro.config.{extension}")),
                format!(
                    "export default {{ fonts: [{{ cssVariable: \"--font-extension-{index}\" }}] }};"
                ),
            )
            .unwrap();
        }
        std::fs::write(
            ignored.join("astro.config.ts"),
            "export default { fonts: [{ cssVariable: \"--font-ignored\" }] };",
        )
        .unwrap();

        let manager = CssVariableManager::new(Config::default());
        let root_uri = Uri::from_file_path(&root).unwrap();
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();

        for index in 0..extensions.len() {
            assert_eq!(
                manager
                    .get_variables(&format!("--font-extension-{index}"))
                    .await
                    .len(),
                1,
                "astro.config extension should be discovered"
            );
        }
        assert!(manager.get_variables("--font-ignored").await.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn ordinary_js_files_are_not_scanned_eagerly() {
        let root =
            std::env::temp_dir().join(format!("css-variable-lsp-js-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("styles.ts"),
            r#"const styles = `--from-js: red;`;"#,
        )
        .unwrap();

        let config = Config {
            lookup_files: vec!["**/*.ts".to_string()],
            ..Config::default()
        };
        let manager = CssVariableManager::new(config);
        let root_uri = Uri::from_file_path(&root).unwrap();

        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();

        assert!(manager.get_variables("--from-js").await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn eager_js_scan_indexes_css_in_all_supported_js_extensions() {
        let root =
            std::env::temp_dir().join(format!("css-variable-lsp-eager-js-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let extensions = ["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"];
        for extension in &extensions {
            std::fs::write(
                root.join(format!("styles.{extension}")),
                format!("const styles = `--eager-{extension}: red;`;"),
            )
            .unwrap();
        }

        let config = Config {
            eager_js: true,
            ..Config::default()
        };
        let manager = CssVariableManager::new(config);
        let root_uri = Uri::from_file_path(&root).unwrap();
        scan_workspace(vec![root_uri], &manager, |_, _| {})
            .await
            .unwrap();

        for extension in &extensions {
            assert_eq!(
                manager
                    .get_variables(&format!("--eager-{extension}"))
                    .await
                    .len(),
                1,
            );
        }

        let mut disabled_config = manager.get_config().await;
        disabled_config.eager_js = false;
        manager.set_config(disabled_config).await;
        scan_workspace(
            vec![Uri::from_file_path(&root).unwrap()],
            &manager,
            |_, _| {},
        )
        .await
        .unwrap();
        for extension in &extensions {
            assert!(manager
                .get_variables(&format!("--eager-{extension}"))
                .await
                .is_empty());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
