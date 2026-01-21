use css_variable_lsp::lsp_server::CssVariableLsp;
use css_variable_lsp::runtime_config::build_runtime_config;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::{timeout, Duration};
use tower::{Service, ServiceExt};
use tower_lsp::jsonrpc::Request;
use tower_lsp::lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, PublishDiagnosticsParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem, Url,
    VersionedTextDocumentIdentifier,
};
use tower_lsp::LspService;

async fn setup_service() -> (LspService<CssVariableLsp>, UnboundedReceiver<Request>) {
    let runtime_config = build_runtime_config(&Vec::new());
    let (service, socket) =
        LspService::new(|client| CssVariableLsp::new(client, runtime_config.clone()));

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut socket = socket;
        while let Some(req) = socket.next().await {
            let _ = tx.send(req);
        }
    });

    (service, rx)
}

async fn send_request(service: &mut LspService<CssVariableLsp>, req: Request) {
    let _ = service.ready().await.unwrap().call(req).await.unwrap();
}

async fn send_notification<P: Serialize>(
    service: &mut LspService<CssVariableLsp>,
    method: &'static str,
    params: P,
) {
    let req = Request::build(method)
        .params(serde_json::to_value(params).unwrap())
        .finish();
    send_request(service, req).await;
}

async fn initialize(service: &mut LspService<CssVariableLsp>) {
    let params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };
    let req = Request::build("initialize")
        .id(1)
        .params(serde_json::to_value(params).unwrap())
        .finish();
    send_request(service, req).await;
}

async fn next_publish_diagnostics_for(
    rx: &mut UnboundedReceiver<Request>,
    uri: &Url,
) -> PublishDiagnosticsParams {
    let result = timeout(Duration::from_secs(2), async {
        loop {
            let req = rx.recv().await.expect("diagnostics channel closed");
            if req.method() != "textDocument/publishDiagnostics" {
                continue;
            }
            let params_value = req.params().cloned().expect("missing diagnostics params");
            let params: PublishDiagnosticsParams =
                serde_json::from_value(params_value).expect("invalid diagnostics payload");
            if &params.uri == uri {
                return params;
            }
        }
    })
    .await;

    result.expect("timed out waiting for diagnostics")
}

async fn open_document(
    service: &mut LspService<CssVariableLsp>,
    uri: Url,
    language_id: &str,
    text: &str,
    version: i32,
) {
    let params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri,
            language_id: language_id.to_string(),
            version,
            text: text.to_string(),
        },
    };
    send_notification(service, "textDocument/didOpen", params).await;
}

async fn change_document(
    service: &mut LspService<CssVariableLsp>,
    uri: Url,
    version: i32,
    new_text: &str,
) {
    let params = DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri, version },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: new_text.to_string(),
        }],
    };
    send_notification(service, "textDocument/didChange", params).await;
}

async fn close_document(service: &mut LspService<CssVariableLsp>, uri: Url) {
    let params = DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri },
    };
    send_notification(service, "textDocument/didClose", params).await;
}

#[tokio::test]
async fn test_diagnostics_revalidate_on_definition_add() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    let vars_uri = Url::parse("file:///vars.scss").unwrap();

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --dark: #000; }",
        1,
    )
    .await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);
}

#[tokio::test]
async fn test_diagnostics_revalidate_on_definition_remove() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    let vars_uri = Url::parse("file:///vars.scss").unwrap();

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --dark: #000; }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);

    change_document(&mut service, vars_uri.clone(), 2, ":root { }").await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
}

#[tokio::test]
async fn test_diagnostics_revalidate_on_definition_close() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    let vars_uri = Url::parse("file:///vars.scss").unwrap();

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --dark: #000; }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);

    close_document(&mut service, vars_uri.clone()).await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
}
