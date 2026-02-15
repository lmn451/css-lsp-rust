use css_variable_lsp::{lsp_server, runtime_config};
use tower_lsp::{LspService, Server};

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

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime_config = runtime_config::build_runtime_config(&args);

    let (service, socket) =
        LspService::new(|client| lsp_server::CssVariableLsp::new(client, runtime_config.clone()));

    Server::new(stdin, stdout, socket).serve(service).await;
}
