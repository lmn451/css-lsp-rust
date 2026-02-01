use css_variable_lsp::lsp_server::CssVariableLsp;
use css_variable_lsp::runtime_config::{build_runtime_config_with_env, RuntimeConfig};
use futures::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::{timeout, Duration};
use tower::{Service, ServiceExt};
use tower_lsp::jsonrpc::Request;
use tower_lsp::lsp_types::{
    ClientCapabilities, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, Url, VersionedTextDocumentIdentifier,
};
use tower_lsp::lsp_types::{
    DeleteFilesParams, DidChangeConfigurationParams, FileDelete, TextDocumentPositionParams,
};
use tower_lsp::LspService;

async fn setup_service_with_config(
    runtime_config: RuntimeConfig,
) -> (LspService<CssVariableLsp>, UnboundedReceiver<Request>) {
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

async fn setup_service() -> (LspService<CssVariableLsp>, UnboundedReceiver<Request>) {
    let runtime_config = build_runtime_config_with_env(&Vec::new(), &HashMap::new());
    setup_service_with_config(runtime_config).await
}

async fn send_request(service: &mut LspService<CssVariableLsp>, req: Request) {
    let _ = service.ready().await.unwrap().call(req).await.unwrap();
}

async fn send_request_for_result(
    service: &mut LspService<CssVariableLsp>,
    req: Request,
) -> Option<serde_json::Value> {
    let response = service.ready().await.unwrap().call(req).await.unwrap();
    response.and_then(|resp| resp.result().cloned())
}

fn position_of(text: &str, needle: &str) -> tower_lsp::lsp_types::Position {
    let offset = text.find(needle).expect("needle should exist in text");
    css_variable_lsp::types::offset_to_position(text, offset)
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

async fn initialize(
    service: &mut LspService<CssVariableLsp>,
) -> tower_lsp::lsp_types::InitializeResult {
    let params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };
    let req = Request::build("initialize")
        .id(1)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let result = send_request_for_result(service, req)
        .await
        .expect("initialize should return result");
    serde_json::from_value(result).expect("initialize result should decode")
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

#[tokio::test]
async fn test_diagnostics_fallback_info_severity() {
    let mut env = HashMap::new();
    env.insert(
        "CSS_LSP_UNDEFINED_VAR_FALLBACK".to_string(),
        "info".to_string(),
    );
    let runtime_config = build_runtime_config_with_env(&Vec::new(), &env);
    let (mut service, mut diagnostics_rx) = setup_service_with_config(runtime_config).await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark, #000); }",
        1,
    )
    .await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(
        diagnostics.diagnostics[0].severity,
        Some(DiagnosticSeverity::INFORMATION)
    );
}

#[tokio::test]
async fn test_initialize_advertises_workspace_folder_change_notifications() {
    let (mut service, _diagnostics_rx) = setup_service().await;

    // This server only advertises workspace folder change notifications if the client
    // declares workspace folder support.
    let req = Request::build("initialize")
        .id(1)
        .params(serde_json::json!({
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true
                }
            }
        }))
        .finish();

    let result = send_request_for_result(&mut service, req)
        .await
        .expect("initialize should return result");
    let init: tower_lsp::lsp_types::InitializeResult =
        serde_json::from_value(result).expect("initialize result should decode");

    let change_notifications = init
        .capabilities
        .workspace
        .and_then(|w| w.workspace_folders)
        .and_then(|wf| wf.change_notifications);

    assert!(matches!(
        change_notifications,
        Some(tower_lsp::lsp_types::OneOf::Left(true))
    ));
}

#[tokio::test]
async fn test_prepare_rename_returns_range() {
    let (mut service, _diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///index.scss").unwrap();
    let text = ".card { color: var(--dark); }";
    open_document(&mut service, uri.clone(), "scss", text, 1).await;

    let pos = position_of(text, "--dark");
    let req = Request::build("textDocument/prepareRename")
        .params(
            serde_json::to_value(TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: pos,
            })
            .unwrap(),
        )
        .id(1)
        .finish();

    let result = send_request_for_result(&mut service, req).await;
    assert!(result.is_some());
}

#[tokio::test]
async fn test_did_change_configuration_disables_color_provider() {
    let (mut service, _diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///colors.scss").unwrap();
    open_document(
        &mut service,
        uri.clone(),
        "scss",
        ":root { --primary: #ff0000; } .x { color: var(--primary); }",
        1,
    )
    .await;

    // Disable color provider via config change.
    let params = DidChangeConfigurationParams {
        settings: serde_json::json!({"enableColorProvider": false}),
    };
    send_notification(&mut service, "workspace/didChangeConfiguration", params).await;

    let req = Request::build("textDocument/documentColor")
        .params(serde_json::json!({"textDocument": {"uri": uri}}))
        .id(2)
        .finish();

    let result = send_request_for_result(&mut service, req).await;
    let colors: Vec<serde_json::Value> = serde_json::from_value(result.unwrap()).unwrap();
    assert_eq!(colors.len(), 0);
}

#[tokio::test]
async fn test_did_delete_files_triggers_revalidation() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    let vars_uri = Url::parse("file:///vars.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --dark: #000; }",
        1,
    )
    .await;

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);

    // Simulate deletion of vars file.
    let params = DeleteFilesParams {
        files: vec![FileDelete {
            uri: vars_uri.to_string(),
        }],
    };
    send_notification(&mut service, "workspace/didDeleteFiles", params).await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
}

#[tokio::test]
async fn test_diagnostics_fallback_off_omits() {
    let mut env = HashMap::new();
    env.insert(
        "CSS_LSP_UNDEFINED_VAR_FALLBACK".to_string(),
        "off".to_string(),
    );
    let runtime_config = build_runtime_config_with_env(&Vec::new(), &env);
    let (mut service, mut diagnostics_rx) = setup_service_with_config(runtime_config).await;
    initialize(&mut service).await;

    let index_uri = Url::parse("file:///index.scss").unwrap();
    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--dark, #000); }",
        1,
    )
    .await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);
}
