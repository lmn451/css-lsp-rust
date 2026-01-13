use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use css_variable_lsp::manager::CssVariableManager;
use css_variable_lsp::types::Config;
use css_variable_lsp::workspace;
use tower_lsp::lsp_types::Url;

fn env_usize(name: &str, default_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn env_f64(name: &str, default_value: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(default_value)
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn ensure_clean_dir(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to remove {path:?}: {e}"))?;
    }
    fs::create_dir_all(path).map_err(|e| format!("Failed to create {path:?}: {e}"))?;
    Ok(())
}

fn color_for(file_index: usize, var_index: usize) -> String {
    let r = ((file_index * 31 + var_index * 3) % 256) as u8;
    let g = ((file_index * 17 + var_index * 7) % 256) as u8;
    let b = ((file_index * 13 + var_index * 11) % 256) as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

fn build_css_content(
    file_index: usize,
    vars_per_file: usize,
    usages_in_file: usize,
    usage_names: &mut Vec<String>,
) -> String {
    let mut content = String::new();
    content.push_str(":root {\n");
    for var_index in 0..vars_per_file {
        let name = format!("--v-c{}-{}", file_index, var_index);
        let color = color_for(file_index, var_index);
        content.push_str("  ");
        content.push_str(&name);
        content.push_str(": ");
        content.push_str(&color);
        content.push_str(";\n");
    }
    content.push_str("}\n\n");
    content.push_str(".class {\n");
    for usage_index in 0..usages_in_file {
        let var_index = if vars_per_file == 0 {
            0
        } else {
            usage_index % vars_per_file
        };
        let name = format!("--v-c{}-{}", file_index, var_index);
        usage_names.push(name.clone());
        content.push_str("  color: var(");
        content.push_str(&name);
        content.push_str(");\n");
    }
    content.push_str("}\n");
    content
}

fn build_html_content(
    file_index: usize,
    vars_per_file: usize,
    usages_in_file: usize,
    usage_names: &mut Vec<String>,
) -> String {
    let mut content = String::new();
    content.push_str("<html><head><style>\n");
    content.push_str(":root {\n");
    for var_index in 0..vars_per_file {
        let name = format!("--v-h{}-{}", file_index, var_index);
        let color = color_for(file_index + 1000, var_index);
        content.push_str("  ");
        content.push_str(&name);
        content.push_str(": ");
        content.push_str(&color);
        content.push_str(";\n");
    }
    content.push_str("}\n");
    content.push_str(".card {\n");
    for usage_index in 0..usages_in_file {
        let var_index = if vars_per_file == 0 {
            0
        } else {
            usage_index % vars_per_file
        };
        let name = format!("--v-h{}-{}", file_index, var_index);
        usage_names.push(name.clone());
        content.push_str("  background-color: var(");
        content.push_str(&name);
        content.push_str(");\n");
    }
    content.push_str("}\n");
    content.push_str("</style></head><body>\n");
    content.push_str("<div class=\"card\"></div>\n");
    content.push_str("</body></html>\n");
    content
}

async fn run() -> Result<(), String> {
    let css_files = env_usize("CSS_LSP_PERF_FILES", 400);
    let html_files = env_usize("CSS_LSP_PERF_HTML_FILES", 50);
    let vars_per_file = env_usize("CSS_LSP_PERF_VARS_PER_FILE", 20);
    let color_usages = env_usize("CSS_LSP_PERF_COLOR_USAGES", 2000);
    let budget_ms_per_file = env_f64("CSS_LSP_PERF_BUDGET_MS_PER_FILE", 15.0);
    let keep_workspace = env_bool("CSS_LSP_PERF_KEEP_WORKSPACE");

    let total_files = css_files + html_files;
    if total_files == 0 {
        return Err("No files requested (CSS_LSP_PERF_FILES + CSS_LSP_PERF_HTML_FILES)".into());
    }

    let workspace_root = PathBuf::from("target").join("perf_workspace");
    ensure_clean_dir(&workspace_root)?;
    let workspace_root = workspace_root
        .canonicalize()
        .map_err(|e| format!("Failed to resolve workspace path: {e}"))?;

    let css_dir = workspace_root.join("styles");
    let html_dir = workspace_root.join("pages");
    fs::create_dir_all(&css_dir).map_err(|e| format!("Failed to create {css_dir:?}: {e}"))?;
    fs::create_dir_all(&html_dir).map_err(|e| format!("Failed to create {html_dir:?}: {e}"))?;

    let usage_base = if total_files > 0 {
        color_usages / total_files
    } else {
        0
    };
    let usage_remainder = if total_files > 0 {
        color_usages % total_files
    } else {
        0
    };

    let mut usage_names = Vec::with_capacity(color_usages);

    let mut file_cursor = 0usize;
    for index in 0..css_files {
        let usages_in_file = usage_base + if file_cursor < usage_remainder { 1 } else { 0 };
        let content = build_css_content(index, vars_per_file, usages_in_file, &mut usage_names);
        let path = css_dir.join(format!("style_{index}.css"));
        fs::write(&path, content).map_err(|e| format!("Failed to write {path:?}: {e}"))?;
        file_cursor += 1;
    }

    for index in 0..html_files {
        let usages_in_file = usage_base + if file_cursor < usage_remainder { 1 } else { 0 };
        let content = build_html_content(index, vars_per_file, usages_in_file, &mut usage_names);
        let path = html_dir.join(format!("page_{index}.html"));
        fs::write(&path, content).map_err(|e| format!("Failed to write {path:?}: {e}"))?;
        file_cursor += 1;
    }

    let config = Config {
        lookup_files: vec!["**/*.css".to_string(), "**/*.html".to_string()],
        ..Default::default()
    };
    let manager = CssVariableManager::new(config);

    let workspace_url = Url::from_directory_path(&workspace_root)
        .map_err(|_| "Invalid workspace path".to_string())?;

    let scan_start = Instant::now();
    workspace::scan_workspace(vec![workspace_url], &manager, |_current, _total| {}).await?;
    let scan_ms = scan_start.elapsed().as_secs_f64() * 1000.0;

    let color_start = Instant::now();
    let mut resolved = 0usize;
    for name in &usage_names {
        if manager.resolve_variable_color(name).await.is_some() {
            resolved += 1;
        }
    }
    let color_ms = color_start.elapsed().as_secs_f64() * 1000.0;

    let ms_per_file = scan_ms / total_files as f64;
    let ms_per_usage = if color_usages > 0 {
        color_ms / color_usages as f64
    } else {
        0.0
    };

    println!("perf config:");
    println!(
        "  files: {} CSS + {} HTML ({} total)",
        css_files, html_files, total_files
    );
    println!("  vars/file: {}", vars_per_file);
    println!("  color usages: {}", color_usages);
    println!("scan: {:.1}ms ({:.2}ms/file)", scan_ms, ms_per_file);
    println!("colors: {:.1}ms (~{:.4}ms/usage)", color_ms, ms_per_usage);
    println!("color resolutions: {}", resolved);

    if ms_per_file <= budget_ms_per_file {
        println!("Result: PASS");
    } else {
        println!(
            "Result: FAIL (scan budget exceeded; budget {:.2}ms/file)",
            budget_ms_per_file
        );
    }

    if !keep_workspace {
        fs::remove_dir_all(&workspace_root)
            .map_err(|e| format!("Failed to remove {workspace_root:?}: {e}"))?;
    } else {
        println!("workspace kept at {}", workspace_root.display());
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("perf failed: {err}");
        std::process::exit(1);
    }
}
