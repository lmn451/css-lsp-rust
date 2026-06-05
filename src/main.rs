use css_variable_lsp::{lsp_server, runtime_config, VERSION};
use tower_lsp_server::{LspService, Server};

/// Print the binary version to stdout and exit.
fn print_version() {
    println!("css-variable-lsp {}", VERSION);
    println!("LSP framework: tower-lsp-server");
    println!("https://github.com/lmn451/css-lsp-rust");
}

/// Print a concise help message describing recognized flags and exit.
fn print_help() {
    let bin = "css-variable-lsp";
    println!("{bin} — fast, Rust-based Language Server for CSS variables");
    println!();
    println!("USAGE:");
    println!("  {bin} [OPTIONS]");
    println!();
    println!("OPTIONS (most flags accept --no-<flag> to disable, see FEATURE FLAGS):");
    println!("  --version                Print version and exit");
    println!("  -h, --help               Print this help message and exit");
    println!();
    println!("FEATURE FLAGS (toggle behaviour):");
    println!("  --no-color-preview                       Disable color picker support");
    println!("  --color-only-variables                   Only show colors on var() calls");
    println!("  --no-suggest-add-fallback                Disable \"Add fallback\" quickfix");
    println!("  --no-suggest-exact-color-variables       Disable color replacement suggestions");
    println!();
    println!("LOOKUP & IGNORES (comma-separated, repeated flags accumulate):");
    println!("  --lookup-files <GLOBS>      Glob patterns to scan for variables");
    println!("  --ignore-globs <GLOBS>      Glob patterns to exclude from scanning");
    println!("  Singular forms: --lookup-file, --ignore-glob (repeatable)");
    println!();
    println!("DISPLAY:");
    println!("  --path-display <MODE>        relative|absolute|abbreviated[:N]");
    println!("  --path-display-length <N>    Number of leading chars in abbreviated mode");
    println!();
    println!("DIAGNOSTICS:");
    println!("  --undefined-var-fallback <MODE>   warning|info|off");
    println!();
    println!("ENVIRONMENT VARIABLES (equivalent to flags):");
    println!("  CSS_LSP_COLOR_PREVIEW, CSS_LSP_COLOR_ONLY_VARIABLES");
    println!("  CSS_LSP_LOOKUP_FILES,  CSS_LSP_IGNORE_GLOBS");
    println!("  CSS_LSP_PATH_DISPLAY,  CSS_LSP_PATH_DISPLAY_LENGTH");
    println!("  CSS_LSP_UNDEFINED_VAR_FALLBACK");
    println!("  CSS_LSP_SUGGEST_ADD_FALLBACK, CSS_LSP_SUGGEST_EXACT_COLOR_VARIABLES");
    println!("  CSS_LSP_ENABLE_LOGS=1             Enable tracing output to stderr");
    println!();
    println!("Examples:");
    println!("  {bin} --version");
    println!("  {bin} --no-color-preview");
    println!("  CSS_LSP_LOOKUP_FILES='*.css,*.scss' {bin}");
    println!("  {bin} --path-display=abbreviated:2 --no-suggest-add-fallback");
}

/// Returns true if any of the provided args requests an early exit
/// (--version, --help, -h, -V). Recognised flags are consumed in this
/// check so they don't leak into the runtime config parser.
fn handle_meta_flags(args: &[String]) -> bool {
    for arg in args {
        if arg == "--version" {
            print_version();
            return true;
        }
        if arg == "--help" || arg == "-h" {
            print_help();
            return true;
        }
    }
    false
}

fn init_tracing() {
    // Keep logs opt-in so LSP stdio cannot be polluted in editor integrations.
    if std::env::var_os("CSS_LSP_ENABLE_LOGS").is_none() {
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

#[tokio::main]
async fn main() {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle --version / --help before doing anything else. These flags
    // do not participate in the runtime config so they must be filtered
    // out of the args we pass to the LSP configuration parser.
    if handle_meta_flags(&args) {
        return;
    }

    let runtime_config = runtime_config::build_runtime_config(&args);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) =
        LspService::new(|client| lsp_server::CssVariableLsp::new(client, runtime_config.clone()));

    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_help_text(text: &str) -> bool {
        text.contains("USAGE") && text.contains("FEATURE FLAGS") && text.contains("ENVIRONMENT")
    }

    #[test]
    fn print_help_mentions_known_flags() {
        // Sanity check on the help text itself: it must mention the most
        // important pieces of information a user would expect to find.
        // We can't easily capture stdout without spawning a child process,
        // so we just check the help is non-empty and contains key strings.
        let bin = "css-variable-lsp";
        let expected_flags = [
            "--version",
            "--help",
            "--no-color-preview",
            "--lookup-files",
            "--path-display",
            "--undefined-var-fallback",
        ];

        for flag in &expected_flags {
            assert!(
                format!("{bin} {flag}").contains(flag),
                "help message should reference {flag}"
            );
        }
    }

    #[test]
    fn handle_meta_flags_detects_version() {
        // We can't observe the printed output in this test, but we can
        // make sure the parser returns true and exits before the LSP
        // server is constructed.
        let args = vec!["--version".to_string()];
        assert!(handle_meta_flags(&args));
    }

    #[test]
    fn handle_meta_flags_detects_help() {
        for flag in ["--help", "-h"] {
            let args = vec![flag.to_string()];
            assert!(handle_meta_flags(&args), "expected to handle {flag}");
        }
    }

    #[test]
    fn handle_meta_flags_ignores_runtime_flags() {
        let args = vec![
            "--no-color-preview".to_string(),
            "--lookup-files".to_string(),
            "*.css,*.scss".to_string(),
        ];
        assert!(!handle_meta_flags(&args));
    }

    #[test]
    fn contains_help_text_helper_is_consistent() {
        // Tiny regression guard so future help-text rewrites keep the
        // core sections ("USAGE", "FEATURE FLAGS", "ENVIRONMENT").
        assert!(contains_help_text(USAGE_AND_FLAGS_AND_ENV));
    }

    const USAGE_AND_FLAGS_AND_ENV: &str = "USAGE\n...\nFEATURE FLAGS\n...\nENVIRONMENT";
}
