use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use regex::Regex;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::{
    ColorInformation, ColorPresentation, ColorPresentationParams, ColorProviderCapability,
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentColorParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    FileChangeType, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, InitializeParams, InitializeResult, Location, MarkupContent, MarkupKind,
    MessageType, OneOf, Position, Range, ReferenceParams, RenameParams, ServerCapabilities,
    SymbolInformation, SymbolKind, TextDocumentContentChangeEvent, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Url, WorkDoneProgressOptions, WorkspaceEdit, WorkspaceFolder,
    WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities, WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer};

use crate::color::{generate_color_presentations, parse_color};
use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document};
use crate::path_display::{format_uri_for_display, to_normalized_fs_path, PathDisplayOptions};
use crate::runtime_config::RuntimeConfig;
use crate::specificity::{
    calculate_specificity, compare_specificity, format_specificity, matches_context,
    sort_by_cascade,
};
use crate::types::{position_to_offset, Config};

pub struct CssVariableLsp {
    client: Client,
    manager: Arc<CssVariableManager>,
    document_map: Arc<RwLock<HashMap<Url, String>>>,
    runtime_config: RuntimeConfig,
    workspace_folder_paths: Arc<RwLock<Vec<PathBuf>>>,
    root_folder_path: Arc<RwLock<Option<PathBuf>>>,
    has_workspace_folder_capability: Arc<RwLock<bool>>,
    has_diagnostic_related_information: Arc<RwLock<bool>>,
    usage_regex: Regex,
    var_usage_regex: Regex,
    var_partial_regex: Regex,
    style_attr_regex: Regex,
}

impl CssVariableLsp {
    pub fn new(client: Client, runtime_config: RuntimeConfig) -> Self {
        let config = Config::from_runtime(&runtime_config);
        Self {
            client,
            manager: Arc::new(CssVariableManager::new(config)),
            document_map: Arc::new(RwLock::new(HashMap::new())),
            runtime_config,
            workspace_folder_paths: Arc::new(RwLock::new(Vec::new())),
            root_folder_path: Arc::new(RwLock::new(None)),
            has_workspace_folder_capability: Arc::new(RwLock::new(false)),
            has_diagnostic_related_information: Arc::new(RwLock::new(false)),
            usage_regex: Regex::new(r"var\((--[\w-]+)(?:\s*,\s*[^)]+)?\)").unwrap(),
            var_usage_regex: Regex::new(r"var\((--[\w-]+)\)").unwrap(),
            var_partial_regex: Regex::new(r"var\(\s*(--[\w-]*)$").unwrap(),
            style_attr_regex: Regex::new(r#"(?i)style\s*=\s*["'][^"']*:\s*[^"';]*$"#).unwrap(),
        }
    }

    async fn update_workspace_folder_paths(&self, folders: Option<Vec<WorkspaceFolder>>) {
        let mut paths = Vec::new();
        if let Some(folders) = folders {
            for folder in folders {
                if let Some(path) = to_normalized_fs_path(&folder.uri) {
                    paths.push(path);
                }
            }
        }
        paths.sort_by_key(|b| std::cmp::Reverse(b.to_string_lossy().len()));
        let mut stored = self.workspace_folder_paths.write().await;
        *stored = paths;
    }

    async fn parse_document_text(&self, uri: &Url, text: &str) {
        self.manager.remove_document(uri).await;

        let path = uri.path().to_lowercase();
        let result = if is_html_like(&path) {
            parse_html_document(text, uri, &self.manager).await
        } else if is_css_like(&path) {
            parse_css_document(text, uri, &self.manager).await
        } else {
            return;
        };

        if let Err(e) = result {
            self.client
                .log_message(MessageType::ERROR, format!("Parse error: {}", e))
                .await;
        }
    }

    async fn validate_document_text(&self, uri: &Url, text: &str) {
        let mut diagnostics = Vec::new();
        let has_related_info = *self.has_diagnostic_related_information.read().await;

        for captures in self.usage_regex.captures_iter(text) {
            let match_all = captures.get(0).unwrap();
            let name = captures.get(1).unwrap().as_str();
            let definitions = self.manager.get_variables(name).await;
            if !definitions.is_empty() {
                continue;
            }
            let range = Range::new(
                crate::types::offset_to_position(text, match_all.start()),
                crate::types::offset_to_position(text, match_all.end()),
            );
            diagnostics.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: None,
                code_description: None,
                source: Some("css-variable-lsp".to_string()),
                message: format!("CSS variable '{}' is not defined in the workspace", name),
                related_information: if has_related_info {
                    Some(Vec::new())
                } else {
                    None
                },
                tags: None,
                data: None,
            });
        }

        self.client
            .publish_diagnostics(uri.clone(), diagnostics, None)
            .await;
    }

    async fn validate_all_open_documents(&self) {
        let docs_snapshot = {
            let docs = self.document_map.read().await;
            docs.iter()
                .map(|(uri, text)| (uri.clone(), text.clone()))
                .collect::<Vec<_>>()
        };

        for (uri, text) in docs_snapshot {
            self.validate_document_text(&uri, &text).await;
        }
    }

    async fn update_document_from_disk(&self, uri: &Url) {
        let path = match to_normalized_fs_path(uri) {
            Some(path) => path,
            None => {
                self.manager.remove_document(uri).await;
                return;
            }
        };

        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                self.parse_document_text(uri, &text).await;
            }
            Err(_) => {
                self.manager.remove_document(uri).await;
            }
        }
    }

    async fn apply_content_changes(
        &self,
        uri: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<String> {
        let mut docs = self.document_map.write().await;
        let mut text = if let Some(existing) = docs.get(uri) {
            existing.clone()
        } else {
            if changes.len() == 1 && changes[0].range.is_none() {
                let new_text = changes[0].text.clone();
                docs.insert(uri.clone(), new_text.clone());
                return Some(new_text);
            }
            return None;
        };

        for change in changes {
            apply_change_to_text(&mut text, &change);
        }

        docs.insert(uri.clone(), text.clone());
        Some(text)
    }

    fn get_word_at_position(&self, text: &str, position: Position) -> Option<String> {
        let offset = position_to_offset(text, position)?;
        let offset = clamp_to_char_boundary(text, offset);
        let before = &text[..offset];
        let after = &text[offset..];

        let left = before
            .rsplit(|c: char| !is_word_char(c))
            .next()
            .unwrap_or("");
        let right = after.split(|c: char| !is_word_char(c)).next().unwrap_or("");
        let word = format!("{}{}", left, right);
        if word.starts_with("--") {
            Some(word)
        } else {
            None
        }
    }

    fn is_in_css_value_context(&self, text: &str, position: Position) -> bool {
        let offset = match position_to_offset(text, position) {
            Some(o) => o,
            None => return false,
        };
        let start = clamp_to_char_boundary(text, offset.saturating_sub(200));
        let offset = clamp_to_char_boundary(text, offset);
        let before_cursor = &text[start..offset];

        if self.var_partial_regex.is_match(before_cursor) {
            return true;
        }

        if let Some(_property_name) = get_property_name_from_context(before_cursor) {
            return true;
        }

        if self.style_attr_regex.is_match(before_cursor) {
            return true;
        }

        false
    }

    fn get_property_name_from_context(&self, text: &str, position: Position) -> Option<String> {
        let offset = position_to_offset(text, position)?;
        let start = clamp_to_char_boundary(text, offset.saturating_sub(200));
        let offset = clamp_to_char_boundary(text, offset);
        let before_cursor = &text[start..offset];
        get_property_name_from_context(before_cursor)
    }

    async fn is_document_open(&self, uri: &Url) -> bool {
        let docs = self.document_map.read().await;
        docs.contains_key(uri)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for CssVariableLsp {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        self.client
            .log_message(MessageType::INFO, "CSS Variable LSP (Rust) initializing...")
            .await;

        let has_workspace_folders = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.workspace_folders)
            .unwrap_or(false);
        let has_related_info = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|t| t.publish_diagnostics.as_ref())
            .and_then(|p| p.related_information)
            .unwrap_or(false);

        {
            let mut cap = self.has_workspace_folder_capability.write().await;
            *cap = has_workspace_folders;
        }
        {
            let mut rel = self.has_diagnostic_related_information.write().await;
            *rel = has_related_info;
        }

        if let Some(root_uri) = params.root_uri.as_ref() {
            let root_path = to_normalized_fs_path(root_uri);
            let mut root = self.root_folder_path.write().await;
            *root = root_path;
        } else {
            #[allow(deprecated)]
            if let Some(root_path) = params.root_path.as_ref() {
                let mut root = self.root_folder_path.write().await;
                *root = Some(PathBuf::from(root_path));
            }
        }

        self.update_workspace_folder_paths(params.workspace_folders.clone())
            .await;

        let mut capabilities = ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(vec!["-".to_string()]),
                work_done_progress_options: WorkDoneProgressOptions::default(),
                all_commit_characters: None,
                completion_item: None,
            }),
            hover_provider: Some(tower_lsp::lsp_types::HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            rename_provider: Some(OneOf::Left(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            color_provider: if self.runtime_config.enable_color_provider {
                Some(ColorProviderCapability::Simple(true))
            } else {
                None
            },
            ..Default::default()
        };

        if has_workspace_folders {
            capabilities.workspace = Some(WorkspaceServerCapabilities {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: None,
                }),
                file_operations: None,
            });
        }

        Ok(InitializeResult {
            capabilities,
            server_info: Some(tower_lsp::lsp_types::ServerInfo {
                name: "css-variable-lsp-rust".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: tower_lsp::lsp_types::InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "CSS Variable LSP (Rust) initialized!")
            .await;

        if let Ok(Some(folders)) = self.client.workspace_folders().await {
            self.update_workspace_folder_paths(Some(folders.clone()))
                .await;
            self.scan_workspace_folders(folders).await;
        }
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        {
            let mut docs = self.document_map.write().await;
            docs.insert(uri.clone(), text.clone());
        }
        self.parse_document_text(&uri, &text).await;
        self.validate_document_text(&uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let changes = params.content_changes;
        let updated_text = match self.apply_content_changes(&uri, changes).await {
            Some(text) => text,
            None => return,
        };
        self.parse_document_text(&uri, &updated_text).await;
        self.validate_document_text(&uri, &updated_text).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut docs = self.document_map.write().await;
            docs.remove(&uri);
        }
        self.update_document_from_disk(&uri).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::DELETED => {
                    self.manager.remove_document(&change.uri).await;
                }
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if !self.is_document_open(&change.uri).await {
                        self.update_document_from_disk(&change.uri).await;
                    }
                }
                _ => {}
            }
        }

        self.validate_all_open_documents().await;
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let mut current_paths = {
            let paths = self.workspace_folder_paths.read().await;
            paths.clone()
        };

        for removed in params.event.removed {
            if let Some(path) = to_normalized_fs_path(&removed.uri) {
                current_paths.retain(|p| p != &path);
            }
        }

        for added in params.event.added {
            if let Some(path) = to_normalized_fs_path(&added.uri) {
                current_paths.push(path);
            }
        }

        current_paths.sort_by_key(|b| std::cmp::Reverse(b.to_string_lossy().len()));

        let mut stored = self.workspace_folder_paths.write().await;
        *stored = current_paths;
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> tower_lsp::jsonrpc::Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(Some(CompletionResponse::Array(Vec::new()))),
        };

        if !self.is_in_css_value_context(&text, position) {
            return Ok(Some(CompletionResponse::Array(Vec::new())));
        }

        let property_name = self.get_property_name_from_context(&text, position);
        let variables = self.manager.get_all_variables().await;

        let mut unique_vars = HashMap::new();
        for var in variables {
            unique_vars.entry(var.name.clone()).or_insert(var);
        }

        let mut scored_vars: Vec<(i32, _)> = unique_vars
            .values()
            .map(|var| {
                let score = score_variable_relevance(&var.name, property_name.as_deref());
                (score, var)
            })
            .collect();

        scored_vars.retain(|(score, _)| *score != 0);
        scored_vars.sort_by(|(score_a, var_a), (score_b, var_b)| {
            if score_a != score_b {
                return score_b.cmp(score_a);
            }
            var_a.name.cmp(&var_b.name)
        });

        let workspace_folder_paths = self.workspace_folder_paths.read().await.clone();
        let root_folder_path = self.root_folder_path.read().await.clone();

        let items = scored_vars
            .into_iter()
            .map(|(_, var)| {
                let options = PathDisplayOptions {
                    mode: self.runtime_config.path_display_mode,
                    abbrev_length: self.runtime_config.path_display_abbrev_length,
                    workspace_folder_paths: &workspace_folder_paths,
                    root_folder_path: root_folder_path.as_ref(),
                };
                CompletionItem {
                    label: var.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(var.value.clone()),
                    documentation: Some(tower_lsp::lsp_types::Documentation::String(format!(
                        "Defined in {}",
                        format_uri_for_display(&var.uri, options)
                    ))),
                    ..Default::default()
                }
            })
            .collect();

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(None),
        };

        let word = match self.get_word_at_position(&text, position) {
            Some(word) => word,
            None => return Ok(None),
        };

        let mut definitions = self.manager.get_variables(&word).await;
        if definitions.is_empty() {
            return Ok(None);
        }

        let usages = self.manager.get_usages(&word).await;
        let offset = match position_to_offset(&text, position) {
            Some(offset) => offset,
            None => return Ok(None),
        };
        let hover_usage = usages.iter().find(|usage| {
            if usage.uri != uri {
                return false;
            }
            let start = position_to_offset(&text, usage.range.start).unwrap_or(0);
            let end = position_to_offset(&text, usage.range.end).unwrap_or(0);
            offset >= start && offset <= end
        });

        let usage_context = hover_usage
            .map(|u| u.usage_context.clone())
            .unwrap_or_default();
        let is_inline_style = usage_context == "inline-style";
        let dom_tree = self.manager.get_dom_tree(&uri).await;
        let dom_node = hover_usage.and_then(|u| u.dom_node.clone());

        sort_by_cascade(&mut definitions);

        let mut hover_text = format!("### CSS Variable: `{}`\n\n", word);

        if definitions.len() == 1 {
            let var = &definitions[0];
            hover_text.push_str(&format!("**Value:** `{}`", var.value));
            if var.important {
                hover_text.push_str(" **!important**");
            }
            hover_text.push_str("\n\n");
            if !var.selector.is_empty() {
                hover_text.push_str(&format!("**Defined in:** `{}`\n", var.selector));
                hover_text.push_str(&format!(
                    "**Specificity:** {}\n",
                    format_specificity(calculate_specificity(&var.selector))
                ));
            }
        } else {
            hover_text.push_str("**Definitions** (CSS cascade order):\n\n");

            for (idx, var) in definitions.iter().enumerate() {
                let spec = calculate_specificity(&var.selector);
                let is_applicable = if usage_context.is_empty() {
                    true
                } else {
                    matches_context(
                        &var.selector,
                        &usage_context,
                        dom_tree.as_ref(),
                        dom_node.as_ref(),
                    )
                };
                let is_winner = idx == 0 && (is_applicable || is_inline_style);

                let mut line = format!("{}. `{}`", idx + 1, var.value);
                if var.important {
                    line.push_str(" **!important**");
                }
                if !var.selector.is_empty() {
                    line.push_str(&format!(
                        " from `{}` {}",
                        var.selector,
                        format_specificity(spec)
                    ));
                }

                if is_winner && !usage_context.is_empty() {
                    if var.important {
                        line.push_str(" ✓ **Wins (!important)**");
                    } else if is_inline_style {
                        line.push_str(" ✓ **Would apply (inline style)**");
                    } else if dom_tree.is_some() && dom_node.is_some() {
                        line.push_str(" ✓ **Applies (DOM match)**");
                    } else {
                        line.push_str(" ✓ **Applies here**");
                    }
                } else if !is_applicable && !usage_context.is_empty() && !is_inline_style {
                    line.push_str(" _(selector doesn't match)_");
                } else if idx > 0 && !usage_context.is_empty() {
                    let winner = &definitions[0];
                    if winner.important && !var.important {
                        line.push_str(" _(overridden by !important)_");
                    } else {
                        let winner_spec = calculate_specificity(&winner.selector);
                        let cmp = compare_specificity(winner_spec, spec);
                        if cmp > 0 {
                            line.push_str(" _(lower specificity)_");
                        } else if cmp == 0 {
                            line.push_str(" _(earlier in source)_");
                        }
                    }
                }

                hover_text.push_str(&line);
                hover_text.push('\n');
            }

            if !usage_context.is_empty() {
                if is_inline_style {
                    hover_text.push_str("\n_Context: Inline style (highest priority)_");
                } else if dom_tree.is_some() && dom_node.is_some() {
                    hover_text.push_str(&format!(
                        "\n_Context: `{}` (DOM-aware matching)_",
                        usage_context
                    ));
                } else {
                    hover_text.push_str(&format!("\n_Context: `{}`_", usage_context));
                }
            }
        }

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text,
            }),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> tower_lsp::jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(None),
        };

        let word = match self.get_word_at_position(&text, position) {
            Some(word) => word,
            None => return Ok(None),
        };

        let definitions = self.manager.get_variables(&word).await;
        let first = match definitions.first() {
            Some(def) => def,
            None => return Ok(None),
        };

        Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
            first.uri.clone(),
            first.range,
        ))))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> tower_lsp::jsonrpc::Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(None),
        };

        let word = match self.get_word_at_position(&text, position) {
            Some(word) => word,
            None => return Ok(None),
        };

        let (definitions, usages) = self.manager.get_references(&word).await;
        let mut locations = Vec::new();
        for def in definitions {
            locations.push(Location::new(def.uri, def.range));
        }
        for usage in usages {
            locations.push(Location::new(usage.uri, usage.range));
        }

        Ok(Some(locations))
    }

    async fn document_color(
        &self,
        params: DocumentColorParams,
    ) -> tower_lsp::jsonrpc::Result<Vec<ColorInformation>> {
        let config = self.manager.get_config().await;
        if !config.enable_color_provider {
            return Ok(Vec::new());
        }

        let uri = params.text_document.uri;
        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(Vec::new()),
        };

        let mut colors = Vec::new();

        if !config.color_only_on_variables {
            let definitions = self.manager.get_document_variables(&uri).await;
            for def in definitions {
                if let Some(color) = parse_color(&def.value) {
                    if let Some(value_range) = def.value_range {
                        colors.push(ColorInformation {
                            range: value_range,
                            color,
                        });
                    } else if let Some(range) = find_value_range_in_definition(&text, &def) {
                        colors.push(ColorInformation { range, color });
                    }
                }
            }
        }

        for caps in self.var_usage_regex.captures_iter(&text) {
            let match_all = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            if let Some(color) = self.manager.resolve_variable_color(var_name).await {
                let range = Range::new(
                    crate::types::offset_to_position(&text, match_all.start()),
                    crate::types::offset_to_position(&text, match_all.end()),
                );
                colors.push(ColorInformation { range, color });
            }
        }

        Ok(colors)
    }

    async fn color_presentation(
        &self,
        params: ColorPresentationParams,
    ) -> tower_lsp::jsonrpc::Result<Vec<ColorPresentation>> {
        if !self.runtime_config.enable_color_provider {
            return Ok(Vec::new());
        }
        Ok(generate_color_presentations(params.color, params.range))
    }

    async fn rename(
        &self,
        params: RenameParams,
    ) -> tower_lsp::jsonrpc::Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(None),
        };

        let old_name = match self.get_word_at_position(&text, position) {
            Some(word) => word,
            None => return Ok(None),
        };

        let (definitions, usages) = self.manager.get_references(&old_name).await;
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

        for def in definitions {
            let range = def.name_range.unwrap_or(def.range);
            changes
                .entry(def.uri.clone())
                .or_default()
                .push(TextEdit {
                    range,
                    new_text: new_name.clone(),
                });
        }

        for usage in usages {
            let range = usage.name_range.unwrap_or(usage.range);
            changes
                .entry(usage.uri.clone())
                .or_default()
                .push(TextEdit {
                    range,
                    new_text: new_name.clone(),
                });
        }

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }

    #[allow(deprecated)]
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> tower_lsp::jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let vars = self
            .manager
            .get_document_variables(&params.text_document.uri)
            .await;
        let symbols: Vec<DocumentSymbol> = vars
            .into_iter()
            .map(|var| DocumentSymbol {
                name: var.name,
                detail: Some(var.value),
                kind: SymbolKind::VARIABLE,
                tags: None,
                deprecated: None,
                range: var.range,
                selection_range: var.range,
                children: None,
            })
            .collect();

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    #[allow(deprecated)]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> tower_lsp::jsonrpc::Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.to_lowercase();
        let vars = self.manager.get_all_variables().await;
        let mut symbols = Vec::new();

        for var in vars {
            if !query.is_empty() && !var.name.to_lowercase().contains(&query) {
                continue;
            }
            symbols.push(SymbolInformation {
                name: var.name,
                kind: SymbolKind::VARIABLE,
                tags: None,
                deprecated: None,
                location: Location::new(var.uri, var.range),
                container_name: None,
            });
        }

        Ok(Some(symbols))
    }
}

impl CssVariableLsp {
    /// Scan workspace folders for CSS and HTML files
    pub async fn scan_workspace_folders(&self, folders: Vec<WorkspaceFolder>) {
        let folder_uris: Vec<Url> = folders.iter().map(|f| f.uri.clone()).collect();

        self.client
            .log_message(
                MessageType::INFO,
                format!("Scanning {} workspace folders...", folder_uris.len()),
            )
            .await;

        let manager = self.manager.clone();
        let client = self.client.clone();

        let mut last_logged_percentage = 0;
        let result = crate::workspace::scan_workspace(folder_uris, &manager, |current, total| {
            if total == 0 {
                return;
            }
            let percentage = ((current as f64 / total as f64) * 100.0).round() as i32;
            if percentage - last_logged_percentage >= 20 || current == total {
                last_logged_percentage = percentage;
                let client = client.clone();
                tokio::spawn(async move {
                    client
                        .log_message(
                            MessageType::INFO,
                            format!(
                                "Scanning CSS files: {}/{} ({}%)",
                                current, total, percentage
                            ),
                        )
                        .await;
                });
            }
        })
        .await;

        match result {
            Ok(_) => {
                let total_vars = manager.get_all_variables().await.len();
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Workspace scan complete. Found {} CSS variables.",
                            total_vars
                        ),
                    )
                    .await;
            }
            Err(e) => {
                self.client
                    .log_message(MessageType::ERROR, format!("Workspace scan failed: {}", e))
                    .await;
            }
        }

        self.validate_all_open_documents().await;
    }
}

fn is_css_like(path: &str) -> bool {
    path.ends_with(".css")
        || path.ends_with(".scss")
        || path.ends_with(".sass")
        || path.ends_with(".less")
}

fn is_html_like(path: &str) -> bool {
    path.ends_with(".html")
        || path.ends_with(".vue")
        || path.ends_with(".svelte")
        || path.ends_with(".astro")
        || path.ends_with(".ripple")
}

fn clamp_to_char_boundary(text: &str, mut idx: usize) -> usize {
    if idx > text.len() {
        idx = text.len();
    }
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn find_context_colon(before_cursor: &str) -> Option<usize> {
    let mut in_braces = 0i32;
    let mut in_parens = 0i32;
    let mut last_colon: i32 = -1;
    let mut last_semicolon: i32 = -1;
    let mut last_brace: i32 = -1;

    for (idx, ch) in before_cursor.char_indices().rev() {
        match ch {
            ')' => in_parens += 1,
            '(' => {
                in_parens -= 1;
                if in_parens < 0 {
                    break;
                }
            }
            '}' => in_braces += 1,
            '{' => {
                in_braces -= 1;
                if in_braces < 0 {
                    last_brace = idx as i32;
                    break;
                }
            }
            ':' if in_parens == 0 && in_braces == 0 && last_colon == -1 => {
                last_colon = idx as i32;
            }
            ';' if in_parens == 0 && in_braces == 0 && last_semicolon == -1 => {
                last_semicolon = idx as i32;
            }
            _ => {}
        }
    }

    if last_colon > last_semicolon && last_colon > last_brace {
        Some(last_colon as usize)
    } else {
        None
    }
}

fn get_property_name_from_context(before_cursor: &str) -> Option<String> {
    let colon_pos = find_context_colon(before_cursor)?;
    let before_colon = before_cursor[..colon_pos].trim_end();
    if before_colon.is_empty() {
        return None;
    }

    let mut start = before_colon.len();
    for (idx, ch) in before_colon.char_indices().rev() {
        if is_word_char(ch) {
            start = idx;
        } else {
            break;
        }
    }

    if start >= before_colon.len() {
        return None;
    }

    Some(before_colon[start..].to_lowercase())
}

fn score_variable_relevance(var_name: &str, property_name: Option<&str>) -> i32 {
    let property_name = match property_name {
        Some(name) => name,
        None => return -1,
    };

    let lower_var_name = var_name.to_lowercase();

    let color_properties = [
        "color",
        "background-color",
        "background",
        "border-color",
        "outline-color",
        "text-decoration-color",
        "fill",
        "stroke",
    ];
    if color_properties.contains(&property_name) {
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
            || lower_var_name.contains("primary")
            || lower_var_name.contains("secondary")
            || lower_var_name.contains("accent")
            || lower_var_name.contains("text")
            || lower_var_name.contains("border")
            || lower_var_name.contains("link")
        {
            return 10;
        }
        if lower_var_name.contains("spacing")
            || lower_var_name.contains("margin")
            || lower_var_name.contains("padding")
            || lower_var_name.contains("size")
            || lower_var_name.contains("width")
            || lower_var_name.contains("height")
            || lower_var_name.contains("font")
            || lower_var_name.contains("weight")
            || lower_var_name.contains("radius")
        {
            return 0;
        }
        return 5;
    }

    let spacing_properties = [
        "margin",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "padding",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "gap",
        "row-gap",
        "column-gap",
    ];
    if spacing_properties.contains(&property_name) {
        if lower_var_name.contains("spacing")
            || lower_var_name.contains("margin")
            || lower_var_name.contains("padding")
            || lower_var_name.contains("gap")
        {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    let size_properties = [
        "width",
        "height",
        "max-width",
        "max-height",
        "min-width",
        "min-height",
        "font-size",
    ];
    if size_properties.contains(&property_name) {
        if lower_var_name.contains("width")
            || lower_var_name.contains("height")
            || lower_var_name.contains("size")
        {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    if property_name.contains("radius") {
        if lower_var_name.contains("radius") || lower_var_name.contains("rounded") {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    let font_properties = ["font-family", "font-weight", "font-style"];
    if font_properties.contains(&property_name) {
        if lower_var_name.contains("font") {
            return 10;
        }
        if lower_var_name.contains("color") || lower_var_name.contains("spacing") {
            return 0;
        }
        return 5;
    }

    -1
}

fn apply_change_to_text(text: &mut String, change: &TextDocumentContentChangeEvent) {
    if let Some(range) = change.range {
        let start = position_to_offset(text, range.start);
        let end = position_to_offset(text, range.end);
        if let (Some(start), Some(end)) = (start, end) {
            if start <= end && end <= text.len() {
                text.replace_range(start..end, &change.text);
                return;
            }
        }
    }
    *text = change.text.clone();
}

fn find_value_range_in_definition(text: &str, def: &crate::types::CssVariable) -> Option<Range> {
    let start = position_to_offset(text, def.range.start)?;
    let end = position_to_offset(text, def.range.end)?;
    if start >= end || end > text.len() {
        return None;
    }
    let def_text = &text[start..end];
    let colon_index = def_text.find(':')?;
    let after_colon = &def_text[colon_index + 1..];
    let value_trim = def.value.trim();
    let value_index = after_colon.find(value_trim)?;

    let absolute_start = start + colon_index + 1 + value_index;
    let absolute_end = absolute_start + value_trim.len();

    Some(Range::new(
        crate::types::offset_to_position(text, absolute_start),
        crate::types::offset_to_position(text, absolute_end),
    ))
}
