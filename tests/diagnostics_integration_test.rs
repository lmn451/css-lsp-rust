use css_variable_lsp::lsp_server::CssVariableLsp;
use css_variable_lsp::runtime_config::{build_runtime_config_with_env, RuntimeConfig};
use futures::StreamExt;
use ls_types::{
    ClientCapabilities, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, Uri, VersionedTextDocumentIdentifier,
};
use ls_types::{
    CodeActionContext, CodeActionParams, CodeActionResponse, DeleteFilesParams,
    DidChangeConfigurationParams, FileDelete, Range, TextDocumentPositionParams,
};
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::{timeout, Duration};
use tower::{Service, ServiceExt};
use tower_lsp_server::jsonrpc::Request;
use tower_lsp_server::LspService;

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

async fn completion_labels(
    service: &mut LspService<CssVariableLsp>,
    uri: Uri,
    position: ls_types::Position,
) -> Vec<String> {
    let req = Request::build("textDocument/completion")
        .id(42)
        .params(serde_json::json!({
            "textDocument": { "uri": uri },
            "position": position
        }))
        .finish();

    let result = send_request_for_result(service, req)
        .await
        .expect("completion should return result");
    let response: ls_types::CompletionResponse = serde_json::from_value(result).unwrap();
    match response {
        ls_types::CompletionResponse::Array(items) => {
            items.into_iter().map(|item| item.label).collect()
        }
        ls_types::CompletionResponse::List(list) => {
            list.items.into_iter().map(|item| item.label).collect()
        }
    }
}

fn position_of(text: &str, needle: &str) -> ls_types::Position {
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

async fn initialize(service: &mut LspService<CssVariableLsp>) -> ls_types::InitializeResult {
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
    uri: &Uri,
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
    uri: Uri,
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
    uri: Uri,
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

async fn close_document(service: &mut LspService<CssVariableLsp>, uri: Uri) {
    let params = DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri },
    };
    send_notification(service, "textDocument/didClose", params).await;
}

#[tokio::test]
async fn test_diagnostics_revalidate_on_definition_add() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();

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
async fn test_diagnostics_accept_variables_after_nested_media_inside_root() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        r#"
        :root {
            --before: #fff;

            @media (prefers-color-scheme: dark) {
                --during: #000;
            }

            --after: #ccc;
        }
        "#,
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: var(--after); }",
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

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();

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

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();

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

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
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
    let init: ls_types::InitializeResult =
        serde_json::from_value(result).expect("initialize result should decode");

    let change_notifications = init
        .capabilities
        .workspace
        .and_then(|w| w.workspace_folders)
        .and_then(|wf| wf.change_notifications);

    assert!(matches!(
        change_notifications,
        Some(ls_types::OneOf::Left(true))
    ));
}

#[tokio::test]
async fn test_prepare_rename_returns_range() {
    let (mut service, _diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Uri::from_str("file:///index.scss").unwrap();
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

    let uri = Uri::from_str("file:///colors.scss").unwrap();
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

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();

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
async fn test_code_actions_for_undefined_variable() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    let _init = initialize(&mut service).await;

    let uri = Uri::from_str("file:///index.scss").unwrap();
    let text = ".card { color: var(--missing); background: var(--missing, #000); }";
    open_document(&mut service, uri.clone(), "scss", text, 1).await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 2);

    let params = CodeActionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range: Range::new(ls_types::Position::new(0, 0), ls_types::Position::new(0, 0)),
        context: CodeActionContext {
            diagnostics: diagnostics.diagnostics,
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/codeAction")
        .id(99)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let result = send_request_for_result(&mut service, req)
        .await
        .expect("codeAction should return result");

    let actions: CodeActionResponse = serde_json::from_value(result).unwrap();

    // We should at least offer "Create --missing in :root".
    let titles: Vec<String> = actions
        .into_iter()
        .filter_map(|a| match a {
            ls_types::CodeActionOrCommand::CodeAction(ca) => Some(ca.title),
            _ => None,
        })
        .collect();

    assert!(titles
        .iter()
        .any(|t| t.contains("Create --missing in :root")));
    assert!(titles.iter().any(|t| t.contains("Add fallback")));
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

    let index_uri = Uri::from_str("file:///index.scss").unwrap();
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

#[tokio::test]
async fn test_will_save_wait_until_returns_no_edits() {
    let (mut service, _diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Uri::from_str("file:///index.scss").unwrap();
    open_document(
        &mut service,
        uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;

    let req = Request::build("textDocument/willSaveWaitUntil")
        .id(100)
        .params(serde_json::json!({
            "textDocument": { "uri": uri },
            "reason": 1
        }))
        .finish();

    let result = send_request_for_result(&mut service, req).await;
    assert_eq!(result, Some(serde_json::Value::Null));
}

#[tokio::test]
async fn test_save_notifications_keep_server_responsive() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Uri::from_str("file:///index.scss").unwrap();
    open_document(
        &mut service,
        uri.clone(),
        "scss",
        ".card { color: var(--dark); }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);

    send_notification(
        &mut service,
        "textDocument/willSave",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "reason": 1
        }),
    )
    .await;

    send_notification(
        &mut service,
        "textDocument/didSave",
        serde_json::json!({
            "textDocument": { "uri": uri }
        }),
    )
    .await;

    change_document(
        &mut service,
        uri.clone(),
        2,
        ".card { --dark: #000; color: var(--dark); }",
    )
    .await;

    // Should not suggest replacing #000 with --dark when #000 is the value of --dark
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);
}

#[tokio::test]
async fn test_literal_color_diagnostic_and_quick_fixes() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();
    let index_uri = Uri::from_str("file:///index.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --white: #fff; --surface: rgb(255 255 255); }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    let text = ".card { color: #ffffff; background: var(--white); }";
    open_document(&mut service, index_uri.clone(), "scss", text, 1).await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
    let diagnostic = &diagnostics.diagnostics[0];
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::INFORMATION));
    assert_eq!(
        diagnostic.code,
        Some(ls_types::NumberOrString::String(
            "css-variable-lsp.literal-color-replaceable".to_string()
        ))
    );
    assert!(diagnostic.message.contains("variables:"));

    let params = CodeActionParams {
        text_document: TextDocumentIdentifier {
            uri: index_uri.clone(),
        },
        range: diagnostic.range,
        context: CodeActionContext {
            diagnostics: diagnostics.diagnostics.clone(),
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/codeAction")
        .id(100)
        .params(serde_json::to_value(params).unwrap())
        .finish();
    let result = send_request_for_result(&mut service, req)
        .await
        .expect("codeAction should return result");
    let actions: CodeActionResponse = serde_json::from_value(result).unwrap();

    let titles: Vec<String> = actions
        .into_iter()
        .filter_map(|a| match a {
            ls_types::CodeActionOrCommand::CodeAction(ca) => Some(ca.title),
            _ => None,
        })
        .collect();
    assert!(titles
        .iter()
        .any(|title| title == "Replace with var(--surface)"));
    assert!(titles
        .iter()
        .any(|title| title == "Replace with var(--white)"));
}

#[tokio::test]
async fn test_literal_color_completion_returns_exact_matches_only() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();
    let index_uri = Uri::from_str("file:///index.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --white: white; --accent: red; --spacing: 1rem; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    let text = ".card { color: #fff; margin: 10px; }";
    open_document(&mut service, index_uri.clone(), "scss", text, 1).await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;

    let color_position = position_of(text, "#fff");
    let labels = completion_labels(&mut service, index_uri.clone(), color_position).await;
    assert!(labels.contains(&"--white".to_string()));
    assert!(!labels.contains(&"--accent".to_string()));
    assert!(!labels.contains(&"--spacing".to_string()));

    let margin_position = position_of(text, ".card");
    let labels = completion_labels(&mut service, index_uri, margin_position).await;
    assert!(!labels.contains(&"--white".to_string()));
}

#[tokio::test]
async fn test_literal_color_revalidation_on_variable_color_change() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();
    let index_uri = Uri::from_str("file:///index.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --white: #fff; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: #fff; }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);

    change_document(
        &mut service,
        vars_uri.clone(),
        2,
        ":root { --white: #000; }",
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 0);
}

#[tokio::test]
async fn test_literal_color_diagnostic_shows_single_variable_name() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();
    let index_uri = Uri::from_str("file:///index.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --primary: #3b82f6; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: #3b82f6; }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
    let diagnostic = &diagnostics.diagnostics[0];
    assert!(diagnostic.message.contains("--primary"));
    assert!(diagnostic
        .message
        .contains("Consider using --primary for this color"));
}

#[tokio::test]
async fn test_literal_color_diagnostic_shows_multiple_variable_names() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///vars.scss").unwrap();
    let index_uri = Uri::from_str("file:///index.scss").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "scss",
        ":root { --white: #fff; --snow: #fff; --ghost: #ffffff; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    open_document(
        &mut service,
        index_uri.clone(),
        "scss",
        ".card { color: #fff; }",
        1,
    )
    .await;
    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &index_uri).await;
    assert_eq!(diagnostics.diagnostics.len(), 1);
    let diagnostic = &diagnostics.diagnostics[0];
    assert!(diagnostic.message.contains("variables:"));
    assert!(diagnostic.message.contains("--white"));
    assert!(diagnostic.message.contains("--snow"));
    assert!(diagnostic.message.contains("--ghost"));
}

#[tokio::test]
async fn test_literal_color_diagnostic_in_js_styled_components() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///theme.css").unwrap();
    let js_uri = Uri::from_str("file:///Button.tsx").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "css",
        ":root { --primary: #3b82f6; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    let js_text = "const Button = styled.button`\\n  color: #3b82f6;\\n`;";
    open_document(&mut service, js_uri.clone(), "typescriptreact", js_text, 1).await;

    let diagnostics = next_publish_diagnostics_for(&mut diagnostics_rx, &js_uri).await;

    let has_literal_diag = diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("primary"));
    assert!(
        has_literal_diag,
        "Expected 'Consider using --primary' diagnostic in JS file, got {:?}",
        diagnostics.diagnostics
    );
}

#[tokio::test]
async fn test_literal_color_exact_match_completion_in_js_styled_components() {
    let (mut service, mut diagnostics_rx) = setup_service().await;
    initialize(&mut service).await;

    let vars_uri = Uri::from_str("file:///theme.css").unwrap();
    let js_uri = Uri::from_str("file:///Button.tsx").unwrap();

    open_document(
        &mut service,
        vars_uri.clone(),
        "css",
        ":root { --primary: #3b82f6; --accent: #3b82f6; --danger: red; }",
        1,
    )
    .await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &vars_uri).await;

    let js_text = "const Button = styled.button`\\n  color: #3b82f6;\\n`;";
    open_document(&mut service, js_uri.clone(), "typescriptreact", js_text, 1).await;
    let _ = next_publish_diagnostics_for(&mut diagnostics_rx, &js_uri).await;

    let color_pos = position_of(js_text, "#3b82f6");
    let labels = completion_labels(&mut service, js_uri.clone(), color_pos).await;

    // Exact-match should return --primary and --accent, but NOT --danger (red)
    assert!(labels.contains(&"--primary".to_string()));
    assert!(labels.contains(&"--accent".to_string()));
    assert!(
        !labels.contains(&"--danger".to_string()),
        "--danger (red) should NOT appear for literal #3b82f6, got {:?}",
        labels
    );
}
