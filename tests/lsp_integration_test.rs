use css_variable_lsp::lsp_server::CssVariableLsp;
use css_variable_lsp::runtime_config::build_runtime_config;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tower::{Service, ServiceExt};
use tower_lsp::jsonrpc::{Request, Response};
use tower_lsp::lsp_types::{
    ClientCapabilities, CompletionItemKind, CompletionParams, CompletionResponse,
    DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    InitializeParams, Location, Position, ReferenceContext, ReferenceParams,
    TextDocumentIdentifier, TextDocumentItem, Url,
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

async fn send_request(service: &mut LspService<CssVariableLsp>, req: Request) -> Option<Response> {
    service.ready().await.unwrap().call(req).await.unwrap()
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

#[tokio::test]
async fn test_lsp_completion_for_variables() {
    let (mut service, _rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///test.css").unwrap();
    let css_content = r#"
        :root {
            --primary-color: #3b82f6;
            --secondary-color: #8b5cf6;
            --spacing: 1rem;
        }
        
        .button {
            --custom-var: var();
        }
    "#;

    open_document(&mut service, uri.clone(), "css", css_content, 1).await;

    let params = CompletionParams {
        text_document_position: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 8,
                character: 25,
            },
        },
        context: None,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/completion")
        .id(2)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let response = send_request(&mut service, req).await.unwrap();
    let result = response.result().unwrap();

    let completion_response: CompletionResponse = serde_json::from_value(result.clone()).unwrap();
    let completion_items = match completion_response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(_) => Vec::new(),
    };

    assert!(completion_items.len() >= 2);

    let variable_names: Vec<String> = completion_items
        .iter()
        .map(|item| item.label.clone())
        .collect();

    assert!(variable_names.contains(&"--primary-color".to_string()));
    assert!(variable_names.contains(&"--secondary-color".to_string()));

    for item in &completion_items {
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
    }
}

#[tokio::test]
async fn test_lsp_hover_for_variable_definition() {
    let (mut service, _rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///test.css").unwrap();
    let css_content = r#"
        :root {
            --primary-color: #3b82f6;
        }
    "#;

    open_document(&mut service, uri.clone(), "css", css_content, 1).await;

    let params = HoverParams {
        text_document_position_params: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 2,
                character: 13,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let req = Request::build("textDocument/hover")
        .id(2)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let response = send_request(&mut service, req).await.unwrap();
    let result = response.result().unwrap();

    let hover: Hover = serde_json::from_value(result.clone()).unwrap();

    if let tower_lsp::lsp_types::HoverContents::Markup(markup) = hover.contents {
        let content_str = markup.value;
        assert!(content_str.contains("--primary-color") || content_str.contains("#3b82f6"));
    }
}

#[ignore]
#[tokio::test]
async fn test_lsp_goto_definition() {
    let (mut service, _rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///test.css").unwrap();
    let css_content = r#"
        :root {
            --primary-color: #3b82f6;
        }
        
        .button {
            background: var(--primary-color);
        }
    "#;

    open_document(&mut service, uri.clone(), "css", css_content, 1).await;

    let params = GotoDefinitionParams {
        text_document_position_params: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 2,
                character: 5,
            },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/definition")
        .id(2)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let response = send_request(&mut service, req).await.unwrap();
    let result = response.result();

    if result.is_none() || result.unwrap().is_null() {
        panic!("Expected goto definition to return location, got null");
    }

    let definition: GotoDefinitionResponse =
        serde_json::from_value(result.unwrap().clone()).unwrap();

    if let GotoDefinitionResponse::Scalar(location) = definition {
        assert_eq!(location.uri, uri);
        assert_eq!(location.range.start.line, 2);
        assert_eq!(location.range.start.character, 13);
    } else {
        panic!("Expected Location response");
    }
}

#[tokio::test]
async fn test_lsp_find_references() {
    let (mut service, _rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///test.css").unwrap();
    let css_content = r#"
        :root {
            --primary-color: #3b82f6;
        }
        
        .button {
            background: var(--primary-color);
        }
        
        .card {
            color: var(--primary-color);
        }
    "#;

    open_document(&mut service, uri.clone(), "css", css_content, 1).await;

    let params = ReferenceParams {
        text_document_position: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 2,
                character: 13,
            },
        },
        context: ReferenceContext {
            include_declaration: true,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/references")
        .id(2)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let response = send_request(&mut service, req).await.unwrap();
    let result = response.result().unwrap();

    let locations: Vec<Location> = serde_json::from_value(result.clone()).unwrap();

    assert_eq!(locations.len(), 3);

    for location in &locations {
        assert_eq!(location.uri, uri);
    }

    let lines: Vec<u32> = locations.iter().map(|loc| loc.range.start.line).collect();
    assert!(lines.contains(&2));
    assert!(lines.contains(&6));
    assert!(lines.contains(&10));
}

#[tokio::test]
async fn test_lsp_completion_no_context() {
    let (mut service, _rx) = setup_service().await;
    initialize(&mut service).await;

    let uri = Url::parse("file:///test.css").unwrap();
    let css_content = r#"
        .button {
            background: blue;
        }
    "#;

    open_document(&mut service, uri.clone(), "css", css_content, 1).await;

    let params = CompletionParams {
        text_document_position: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 2,
                character: 25,
            },
        },
        context: None,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let req = Request::build("textDocument/completion")
        .id(2)
        .params(serde_json::to_value(params).unwrap())
        .finish();

    let response = send_request(&mut service, req).await.unwrap();
    let result = response.result().unwrap();

    let completion_response: CompletionResponse = serde_json::from_value(result.clone()).unwrap();
    let completion_items = match completion_response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(_) => Vec::new(),
    };

    assert_eq!(completion_items.len(), 0);
}
