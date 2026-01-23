use std::process::Command;
use std::time::Duration;

#[tokio::test]
async fn test_perf_binary_basic_run() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "2")
        .env("CSS_LSP_PERF_HTML_FILES", "1")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "3")
        .env("CSS_LSP_PERF_COLOR_USAGES", "10")
        .env("CSS_LSP_PERF_BUDGET_MS_PER_FILE", "50.0")
        .env("CSS_LSP_PERF_KEEP_WORKSPACE", "1")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();
    let stderr = String::from_utf8(&output.stderr).unwrap();

    assert!(stdout.contains("perf config:"));
    assert!(stdout.contains("files: 2 CSS + 1 HTML (3 total)"));
    assert!(stdout.contains("vars/file: 3"));
    assert!(stdout.contains("color usages: 10"));
    assert!(stdout.contains("scan:"));
    assert!(stdout.contains("colors:"));
    assert!(stdout.contains("Result:"));

    assert!(stderr.is_empty());
}

#[tokio::test]
async fn test_perf_binary_zero_files() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "0")
        .env("CSS_LSP_PERF_HTML_FILES", "0")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(!output.status.success());

    let stderr = String::from_utf8(&output.stderr).unwrap();
    assert!(stderr.contains("No files requested"));
}

#[tokio::test]
async fn test_perf_binary_performance_failure() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "10")
        .env("CSS_LSP_PERF_HTML_FILES", "5")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "5")
        .env("CSS_LSP_PERF_COLOR_USAGES", "100")
        .env("CSS_LSP_PERF_BUDGET_MS_PER_FILE", "0.1")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();

    assert!(stdout.contains("Result: FAIL"));
    assert!(stdout.contains("scan budget exceeded"));
}

#[tokio::test]
async fn test_perf_binary_keep_workspace() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "1")
        .env("CSS_LSP_PERF_HTML_FILES", "1")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "2")
        .env("CSS_LSP_PERF_COLOR_USAGES", "5")
        .env("CSS_LSP_PERF_KEEP_WORKSPACE", "1")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();

    assert!(stdout.contains("workspace kept at"));

    let target_dir = workspace_path.join("target").join("perf_workspace");
    assert!(target_dir.exists());
}

#[tokio::test]
async fn test_perf_binary_environment_variables() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "5")
        .env("CSS_LSP_PERF_HTML_FILES", "3")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "10")
        .env("CSS_LSP_PERF_COLOR_USAGES", "50")
        .env("CSS_LSP_PERF_BUDGET_MS_PER_FILE", "25.5")
        .env("CSS_LSP_PERF_KEEP_WORKSPACE", "0")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();

    assert!(stdout.contains("files: 5 CSS + 3 HTML (8 total)"));
    assert!(stdout.contains("vars/file: 10"));
    assert!(stdout.contains("color usages: 50"));
    assert!(stdout.contains("Result: PASS"));
}

#[tokio::test]
async fn test_perf_binary_color_resolution_counting() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "2")
        .env("CSS_LSP_PERF_HTML_FILES", "0")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "5")
        .env("CSS_LSP_PERF_COLOR_USAGES", "20")
        .env("CSS_LSP_PERF_BUDGET_MS_PER_FILE", "100.0")
        .current_dir(workspace_path)
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();

    assert!(stdout.contains("colors:"));
    assert!(stdout.contains("~"));
    assert!(stdout.contains("/usage)"));
    assert!(stdout.contains("color resolutions:"));

    assert!(stdout.contains("color resolutions:"));
}

#[tokio::test]
async fn test_perf_binary_large_scale() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path();

    let output = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("perf")
        .env("CSS_LSP_PERF_FILES", "50")
        .env("CSS_LSP_PERF_HTML_FILES", "10")
        .env("CSS_LSP_PERF_VARS_PER_FILE", "15")
        .env("CSS_LSP_PERF_COLOR_USAGES", "500")
        .env("CSS_LSP_PERF_BUDGET_MS_PER_FILE", "20.0")
        .env("CSS_LSP_PERF_KEEP_WORKSPACE", "0")
        .current_dir(workspace_path)
        .timeout(Duration::from_secs(120))
        .output()
        .expect("Failed to run perf binary");

    assert!(output.status.success());

    let stdout = String::from_utf8(&output.stdout).unwrap();

    assert!(stdout.contains("files: 50 CSS + 10 HTML (60 total)"));
    assert!(stdout.contains("vars/file: 15"));
    assert!(stdout.contains("color usages: 500"));
    assert!(stdout.contains("Result:"));
}
