use css_variable_lsp::{lsp_server, runtime_config};
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    // Initialize tracing for debugging
    tracing_subscriber::fmt::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime_config = runtime_config::build_runtime_config(&args);

    let (service, socket) =
        LspService::new(|client| lsp_server::CssVariableLsp::new(client, runtime_config.clone()));

    Server::new(stdin, stdout, socket).serve(service).await;
}
