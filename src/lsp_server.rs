use ls_types::Uri;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use ls_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, ColorInformation, ColorPresentation,
    ColorPresentationParams, ColorProviderCapability, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, CreateFilesParams, DeleteFilesParams,
    Diagnostic, DiagnosticSeverity, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentColorParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, InitializeParams, InitializeResult,
    Location, MarkupContent, MarkupKind, MessageType, OneOf, Position, PrepareRenameResponse,
    Range, ReferenceParams, RenameFilesParams, RenameOptions, RenameParams, ServerCapabilities,
    SymbolInformation, SymbolKind, TextDocumentContentChangeEvent, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, WillSaveTextDocumentParams,
    WorkDoneProgressOptions, WorkspaceEdit, WorkspaceFolder, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use regex::Regex;
use tokio::sync::RwLock;
use tower_lsp_server::{Client, LanguageServer};

use crate::color::{generate_color_presentations, parse_color, NormalizedColorKey};
use crate::manager::CssVariableManager;
use crate::parsers::{parse_css_document, parse_html_document};
use crate::path_display::{format_uri_for_display, to_normalized_fs_path, PathDisplayOptions};
use crate::runtime_config::{RuntimeConfig, UndefinedVarFallbackMode};
use crate::specificity::{
    calculate_specificity, compare_specificity, format_specificity, matches_context,
    sort_by_cascade,
};
use crate::types::{position_to_offset, Config};

fn code_actions_for_undefined_variables(
    uri: &Uri,
    text: &str,
    context: &CodeActionContext,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();

    for diag in &context.diagnostics {
        let code = match diag.code.as_ref() {
            Some(ls_types::NumberOrString::String(code)) => code.as_str(),
            _ => continue,
        };
        if code != "css-variable-lsp.undefined-variable" {
            continue;
        }

        let name = diag
            .data
            .as_ref()
            .and_then(|d| d.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let name = match name {
            Some(name) => name,
            None => continue,
        };

        // Very conservative quickfix: insert a :root block at the start of the current file.
        // This avoids trying to parse/modify existing CSS.
        let insert_text = format!(":root {{\n    {}: ;\n}}\n\n", name);

        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri.clone(),
                vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    new_text: insert_text,
                }],
            )])),
            document_changes: None,
            change_annotations: None,
        };

        let action = CodeAction {
            title: format!("Create {} in :root", name),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diag.clone()]),
            edit: Some(edit),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        };

        actions.push(CodeActionOrCommand::CodeAction(action));

        // Optional quickfix: add fallback to `var(--name)` -> `var(--name, )`
        // Only offered when the diagnostic covers a `var(...)` call without a comma.
        if let (Some(start), Some(end)) = (
            crate::types::position_to_offset(text, diag.range.start),
            crate::types::position_to_offset(text, diag.range.end),
        ) {
            if start < end && end <= text.len() {
                let slice = &text[start..end];
                if slice.starts_with("var(") && slice.ends_with(')') && !slice.contains(',') {
                    let new_text = slice.trim_end_matches(')').to_string() + ", )";
                    let edit = WorkspaceEdit {
                        changes: Some(HashMap::from([(
                            uri.clone(),
                            vec![TextEdit {
                                range: diag.range,
                                new_text,
                            }],
                        )])),
                        document_changes: None,
                        change_annotations: None,
                    };

                    let action = CodeAction {
                        title: format!("Add fallback to {}", name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(edit),
                        command: None,
                        is_preferred: Some(false),
                        disabled: None,
                        data: None,
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
        }
    }

    actions
}

fn code_actions_for_replaceable_literal_colors(
    uri: &Uri,
    context: &CodeActionContext,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();

    for diag in &context.diagnostics {
        let code = match diag.code.as_ref() {
            Some(ls_types::NumberOrString::String(code)) => code.as_str(),
            _ => continue,
        };
        if code != "css-variable-lsp.literal-color-replaceable" {
            continue;
        }

        let replacements = diag
            .data
            .as_ref()
            .and_then(|d| d.get("replacements"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for replacement in replacements {
            let name = match replacement.get("name").and_then(|v| v.as_str()) {
                Some(name) => name,
                None => continue,
            };

            let edit = WorkspaceEdit {
                changes: Some(HashMap::from([(
                    uri.clone(),
                    vec![TextEdit {
                        range: diag.range,
                        new_text: format!("var({})", name),
                    }],
                )])),
                document_changes: None,
                change_annotations: None,
            };

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Replace with var({})", name),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(edit),
                command: None,
                is_preferred: Some(false),
                disabled: None,
                data: None,
            }));
        }
    }

    actions
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientConfigPatch {
    lookup_files: Option<Vec<String>>,
    ignore_globs: Option<Vec<String>>,
    enable_color_provider: Option<bool>,
    color_only_on_variables: Option<bool>,
}

fn apply_config_patch(mut base: Config, patch: ClientConfigPatch) -> Config {
    if let Some(lookup_files) = patch.lookup_files {
        base.lookup_files = lookup_files;
    }
    if let Some(ignore_globs) = patch.ignore_globs {
        base.ignore_globs = ignore_globs;
    }
    if let Some(enable_color_provider) = patch.enable_color_provider {
        base.enable_color_provider = enable_color_provider;
    }
    if let Some(color_only_on_variables) = patch.color_only_on_variables {
        base.color_only_on_variables = color_only_on_variables;
    }
    base
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentKind {
    Css,
    Html,
}

pub struct CssVariableLsp {
    client: Client,
    manager: Arc<CssVariableManager>,
    document_map: Arc<RwLock<HashMap<Uri, String>>>,
    runtime_config: RuntimeConfig,
    workspace_folder_paths: Arc<RwLock<Vec<PathBuf>>>,
    root_folder_path: Arc<RwLock<Option<PathBuf>>>,
    has_workspace_folder_capability: Arc<RwLock<bool>>,
    has_diagnostic_related_information: Arc<RwLock<bool>>,
    usage_regex: Regex,
    var_usage_regex: Regex,
    lookup_extension_map: Arc<RwLock<HashMap<String, DocumentKind>>>,
    live_config: Arc<RwLock<Config>>,
    document_language_map: Arc<RwLock<HashMap<Uri, String>>>,
    document_usage_map: Arc<RwLock<HashMap<Uri, HashSet<String>>>>,
    usage_index: Arc<RwLock<HashMap<String, HashSet<Uri>>>>,
}

impl CssVariableLsp {
    pub fn new(client: Client, runtime_config: RuntimeConfig) -> Self {
        let config = Config::from_runtime(&runtime_config);
        let lookup_extension_map = build_lookup_extension_map(&config.lookup_files);
        let live_config = config.clone();
        Self {
            client,
            manager: Arc::new(CssVariableManager::new(config)),
            document_map: Arc::new(RwLock::new(HashMap::new())),
            runtime_config,
            workspace_folder_paths: Arc::new(RwLock::new(Vec::new())),
            root_folder_path: Arc::new(RwLock::new(None)),
            has_workspace_folder_capability: Arc::new(RwLock::new(false)),
            has_diagnostic_related_information: Arc::new(RwLock::new(false)),
            usage_regex: Regex::new(r"var\((--[\w-]+)(?:\s*,\s*([^)]+))?\)").unwrap(),
            var_usage_regex: Regex::new(r"var\((--[\w-]+)\)").unwrap(),
            lookup_extension_map: Arc::new(RwLock::new(lookup_extension_map)),
            live_config: Arc::new(RwLock::new(live_config)),
            document_language_map: Arc::new(RwLock::new(HashMap::new())),
            document_usage_map: Arc::new(RwLock::new(HashMap::new())),
            usage_index: Arc::new(RwLock::new(HashMap::new())),
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

    async fn parse_document_text(&self, uri: &Uri, text: &str, language_id: Option<&str>) {
        self.manager.remove_document(uri).await;

        let path = uri.path().as_str();
        let lookup_map = self.lookup_extension_map.read().await.clone();
        let kind = resolve_document_kind(path, language_id, &lookup_map);
        let result = match kind {
            Some(DocumentKind::Html) => parse_html_document(text, uri, &self.manager).await,
            Some(DocumentKind::Css) => parse_css_document(text, uri, &self.manager).await,
            None => return,
        };

        if let Err(e) = result {
            self.client
                .log_message(MessageType::ERROR, format!("Parse error: {}", e))
                .await;
        }

        self.manager.rebuild_color_index().await;
    }

    async fn validate_document_text(&self, uri: &Uri, text: &str) {
        let has_related_info = *self.has_diagnostic_related_information.read().await;
        validate_document_text_with(
            &self.client,
            self.manager.as_ref(),
            &self.usage_regex,
            self.runtime_config.undefined_var_fallback,
            has_related_info,
            uri,
            text,
            &self.document_usage_map,
            &self.usage_index,
        )
        .await;
    }

    async fn validate_all_open_documents(&self) {
        let has_related_info = *self.has_diagnostic_related_information.read().await;
        let docs_snapshot = {
            let docs = self.document_map.read().await;
            docs.iter()
                .map(|(uri, text)| (uri.clone(), text.clone()))
                .collect::<Vec<_>>()
        };

        for (uri, text) in docs_snapshot {
            validate_document_text_with(
                &self.client,
                self.manager.as_ref(),
                &self.usage_regex,
                self.runtime_config.undefined_var_fallback,
                has_related_info,
                &uri,
                &text,
                &self.document_usage_map,
                &self.usage_index,
            )
            .await;
        }
    }

    async fn update_document_from_disk(&self, uri: &Uri) {
        let path = match to_normalized_fs_path(uri) {
            Some(path) => path,
            None => {
                self.manager.remove_document(uri).await;
                return;
            }
        };

        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                self.parse_document_text(uri, &text, None).await;
            }
            Err(_) => {
                self.manager.remove_document(uri).await;
                self.manager.rebuild_color_index().await;
            }
        }
    }

    async fn apply_content_changes(
        &self,
        uri: &Uri,
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

    async fn is_document_open(&self, uri: &Uri) -> bool {
        let docs = self.document_map.read().await;
        docs.contains_key(uri)
    }
}

// async_trait macro no longer needed for tower-lsp-server v0.21+
impl LanguageServer for CssVariableLsp {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp_server::jsonrpc::Result<InitializeResult> {
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

        #[allow(deprecated)]
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
                trigger_characters: Some(vec!["-".to_string(), "(".to_string(), ":".to_string()]),
                work_done_progress_options: WorkDoneProgressOptions::default(),
                all_commit_characters: None,
                completion_item: None,
            }),
            hover_provider: Some(ls_types::HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),

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
                    change_notifications: Some(OneOf::Left(true)),
                }),
                file_operations: None,
            });
        }

        Ok(InitializeResult {
            capabilities,
            offset_encoding: None,
            server_info: Some(ls_types::ServerInfo {
                name: "css-variable-lsp-rust".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: ls_types::InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "CSS Variable LSP (Rust) initialized!")
            .await;

        if let Ok(Some(folders)) = self.client.workspace_folders().await {
            self.update_workspace_folder_paths(Some(folders.clone()))
                .await;
            self.scan_workspace_folders(folders).await;
        }
    }

    async fn shutdown(&self) -> tower_lsp_server::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // We accept either:
        // - a flat object matching Config fields
        // - or a namespaced object { "cssVariableLsp": { ... } }
        let patch =
            serde_json::from_value::<ClientConfigPatch>(params.settings.clone()).or_else(|_| {
                params
                    .settings
                    .get("cssVariableLsp")
                    .cloned()
                    .ok_or_else(|| {
                        serde_json::Error::io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing cssVariableLsp key",
                        ))
                    })
                    .and_then(serde_json::from_value::<ClientConfigPatch>)
            });

        let patch = match patch {
            Ok(patch) => patch,
            Err(err) => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("Failed to parse didChangeConfiguration settings: {}", err),
                    )
                    .await;
                return;
            }
        };

        let mut config = self.live_config.read().await.clone();
        let prev_lookup_files = config.lookup_files.clone();
        config = apply_config_patch(config, patch);

        {
            let mut stored = self.live_config.write().await;
            *stored = config.clone();
        }

        self.manager.set_config(config.clone()).await;

        // Update extension map if lookup patterns changed.
        if config.lookup_files != prev_lookup_files {
            let new_map = build_lookup_extension_map(&config.lookup_files);
            let mut stored = self.lookup_extension_map.write().await;
            *stored = new_map;

            // Patterns changed => rescan workspace folders.
            if let Ok(Some(folders)) = self.client.workspace_folders().await {
                self.scan_workspace_folders(folders).await;
            }
        }

        // Always revalidate open docs (diagnostics may change due to ignore patterns etc.).
        self.validate_all_open_documents().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let language_id = params.text_document.language_id;

        let old_names = self.manager.get_document_variable_names(&uri).await;
        let old_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        {
            let mut docs = self.document_map.write().await;
            docs.insert(uri.clone(), text.clone());
        }
        {
            let mut langs = self.document_language_map.write().await;
            langs.insert(uri.clone(), language_id.clone());
        }
        self.parse_document_text(&uri, &text, Some(&language_id))
            .await;

        let new_names = self.manager.get_document_variable_names(&uri).await;
        let new_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        self.validate_document_text(&uri, &text).await;

        if old_names != new_names || old_colors != new_colors {
            self.validate_all_open_documents().await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let changes = params.content_changes;

        let old_names = self.manager.get_document_variable_names(&uri).await;
        let old_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        let updated_text = match self.apply_content_changes(&uri, changes).await {
            Some(text) => text,
            None => return,
        };
        let language_id = {
            let langs = self.document_language_map.read().await;
            langs.get(&uri).cloned()
        };
        self.parse_document_text(&uri, &updated_text, language_id.as_deref())
            .await;

        let new_names = self.manager.get_document_variable_names(&uri).await;
        let new_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        self.validate_document_text(&uri, &updated_text).await;

        if old_names != new_names || old_colors != new_colors {
            self.validate_all_open_documents().await;
        }
    }

    async fn will_save(&self, _params: WillSaveTextDocumentParams) {
        // No-op: no pre-save mutation required.
    }

    async fn will_save_wait_until(
        &self,
        _params: WillSaveTextDocumentParams,
    ) -> tower_lsp_server::jsonrpc::Result<Option<Vec<TextEdit>>> {
        // No-op: no pre-save edits to apply.
        Ok(None)
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        // No-op: we already parse and validate on open/change notifications.
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        let old_names = self.manager.get_document_variable_names(&uri).await;
        let old_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        {
            let mut docs = self.document_map.write().await;
            docs.remove(&uri);
        }
        {
            let mut langs = self.document_language_map.write().await;
            langs.remove(&uri);
        }

        // Clean up usage maps
        {
            let mut usage_map = self.document_usage_map.write().await;
            if let Some(old_usages) = usage_map.remove(&uri) {
                let mut index = self.usage_index.write().await;
                for name in old_usages {
                    if let Some(uris) = index.get_mut(&name) {
                        uris.remove(&uri);
                        if uris.is_empty() {
                            index.remove(&name);
                        }
                    }
                }
            }
        }

        self.update_document_from_disk(&uri).await;

        let new_names = self.manager.get_document_variable_names(&uri).await;
        let new_colors = self.manager.get_document_resolved_color_keys(&uri).await;

        if old_names != new_names || old_colors != new_colors {
            self.validate_all_open_documents().await;
        }
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

    async fn did_create_files(&self, params: CreateFilesParams) {
        for file in params.files {
            let uri = match Uri::from_str(&file.uri) {
                Ok(uri) => uri,
                Err(_) => continue,
            };
            if !self.is_document_open(&uri).await {
                self.update_document_from_disk(&uri).await;
            }
        }
        self.validate_all_open_documents().await;
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        for file in params.files {
            let old_uri = match Uri::from_str(&file.old_uri) {
                Ok(uri) => uri,
                Err(_) => continue,
            };
            let new_uri = match Uri::from_str(&file.new_uri) {
                Ok(uri) => uri,
                Err(_) => continue,
            };

            // Remove old document data
            self.manager.remove_document(&old_uri).await;

            // If the new URI is not open, load it from disk.
            if !self.is_document_open(&new_uri).await {
                self.update_document_from_disk(&new_uri).await;
            }
        }
        self.validate_all_open_documents().await;
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        for file in params.files {
            let uri = match Uri::from_str(&file.uri) {
                Ok(uri) => uri,
                Err(_) => continue,
            };
            self.manager.remove_document(&uri).await;
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
    ) -> tower_lsp_server::jsonrpc::Result<Option<CompletionResponse>> {
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

        let language_id = {
            let langs = self.document_language_map.read().await;
            langs.get(&uri).cloned()
        };
        let lookup_map = self.lookup_extension_map.read().await.clone();
        let context = completion_value_context_slice(
            &text,
            position,
            language_id.as_deref(),
            &uri,
            &lookup_map,
        );
        let context = match context {
            Some(context) => context,
            None => return Ok(Some(CompletionResponse::Array(Vec::new()))),
        };
        let value_context = get_value_context_info(context.slice, context.allow_without_braces);
        if !value_context.is_value_context {
            return Ok(Some(CompletionResponse::Array(Vec::new())));
        }
        let property_name = value_context.property_name;
        let in_var_context = is_var_function_context_slice(context.slice);
        let workspace_folder_paths = self.workspace_folder_paths.read().await.clone();
        let root_folder_path = self.root_folder_path.read().await.clone();

        if !in_var_context {
            if let Some(color_key) = self.literal_color_under_cursor(&uri, position).await {
                let variables = self.manager.get_variables_by_color_key(&color_key).await;
                let items = variables
                    .into_iter()
                    .map(|var| {
                        let options = PathDisplayOptions {
                            mode: self.runtime_config.path_display_mode,
                            abbrev_length: self.runtime_config.path_display_abbrev_length,
                            workspace_folder_paths: &workspace_folder_paths,
                            root_folder_path: root_folder_path.as_ref(),
                        };
                        let display_path = format_uri_for_display(&var.uri, options);
                        CompletionItem {
                            label: var.name.clone(),
                            kind: Some(CompletionItemKind::VARIABLE),
                            detail: Some(format!("{} • {}", var.value, display_path)),
                            insert_text: Some(format!("var({})", var.name)),
                            documentation: Some(ls_types::Documentation::String(format!(
                                "Defined in {}",
                                display_path
                            ))),
                            ..Default::default()
                        }
                    })
                    .collect();
                return Ok(Some(CompletionResponse::Array(items)));
            }
        }

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

        let items = scored_vars
            .into_iter()
            .map(|(_, var)| {
                let options = PathDisplayOptions {
                    mode: self.runtime_config.path_display_mode,
                    abbrev_length: self.runtime_config.path_display_abbrev_length,
                    workspace_folder_paths: &workspace_folder_paths,
                    root_folder_path: root_folder_path.as_ref(),
                };
                let insert_text = if in_var_context {
                    var.name.clone()
                } else {
                    format!("var({})", var.name)
                };
                CompletionItem {
                    label: var.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(var.value.clone()),
                    insert_text: Some(insert_text),
                    documentation: Some(ls_types::Documentation::String(format!(
                        "Defined in {}",
                        format_uri_for_display(&var.uri, options)
                    ))),
                    ..Default::default()
                }
            })
            .collect();

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn completion_resolve(
        &self,
        item: CompletionItem,
    ) -> tower_lsp_server::jsonrpc::Result<CompletionItem> {
        Ok(item)
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp_server::jsonrpc::Result<Option<Hover>> {
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
    ) -> tower_lsp_server::jsonrpc::Result<Option<GotoDefinitionResponse>> {
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

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> tower_lsp_server::jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(Some(Vec::new())),
        };

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // 1) Quick-fix undefined variable diagnostics.
        actions.extend(code_actions_for_undefined_variables(
            &uri,
            &text,
            &params.context,
        ));
        actions.extend(code_actions_for_replaceable_literal_colors(
            &uri,
            &params.context,
        ));

        Ok(Some(actions))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> tower_lsp_server::jsonrpc::Result<Option<Vec<Location>>> {
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
    ) -> tower_lsp_server::jsonrpc::Result<Vec<ColorInformation>> {
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
        let mut seen_ranges: HashSet<(u32, u32, u32, u32)> = HashSet::new();
        let range_key = |range: &Range| {
            (
                range.start.line,
                range.start.character,
                range.end.line,
                range.end.character,
            )
        };

        if !config.color_only_on_variables {
            let definitions = self.manager.get_document_variables(&uri).await;
            for def in definitions {
                if let Some(color) = parse_color(&def.value) {
                    if let Some(value_range) = def.value_range {
                        if seen_ranges.insert(range_key(&value_range)) {
                            colors.push(ColorInformation {
                                range: value_range,
                                color,
                            });
                        }
                    } else if let Some(range) = find_value_range_in_definition(&text, &def) {
                        if seen_ranges.insert(range_key(&range)) {
                            colors.push(ColorInformation { range, color });
                        }
                    }
                }
            }
        }

        let usages = self.manager.get_document_usages(&uri).await;
        for usage in usages {
            if let Some(color) = self.manager.resolve_variable_color(&usage.name).await {
                if seen_ranges.insert(range_key(&usage.range)) {
                    colors.push(ColorInformation {
                        range: usage.range,
                        color,
                    });
                }
            }
        }

        for caps in self.var_usage_regex.captures_iter(&text) {
            let match_all = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            let range = Range::new(
                crate::types::offset_to_position(&text, match_all.start()),
                crate::types::offset_to_position(&text, match_all.end()),
            );
            if !seen_ranges.insert(range_key(&range)) {
                continue;
            }
            if let Some(color) = self.manager.resolve_variable_color(var_name).await {
                colors.push(ColorInformation { range, color });
            }
        }

        Ok(colors)
    }

    async fn color_presentation(
        &self,
        params: ColorPresentationParams,
    ) -> tower_lsp_server::jsonrpc::Result<Vec<ColorPresentation>> {
        if !self.runtime_config.enable_color_provider {
            return Ok(Vec::new());
        }
        Ok(generate_color_presentations(params.color, params.range))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> tower_lsp_server::jsonrpc::Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;

        let text = {
            let docs = self.document_map.read().await;
            docs.get(&uri).cloned()
        };
        let text = match text {
            Some(text) => text,
            None => return Ok(None),
        };

        let name = match self.get_word_at_position(&text, position) {
            Some(name) => name,
            None => return Ok(None),
        };

        // Prefer the precise name range when available.
        let definitions = self.manager.get_variables(&name).await;
        let range = definitions
            .first()
            .and_then(|d| d.name_range)
            .unwrap_or_else(|| {
                // Fallback to the word bounds at the cursor position.
                // We intentionally return the cursor word selection rather than the whole declaration.
                let offset = position_to_offset(&text, position).unwrap_or(0);
                let offset = clamp_to_char_boundary(&text, offset);

                let before = &text[..offset];
                let after = &text[offset..];

                // Compute byte indices for the word under the cursor.
                // We do this manually because LSP positions are UTF-16 based.
                //
                // Note: get_word_at_position already verifies we're on a `--var`.
                // So the scan boundaries are safe for our token definition.

                // Simpler: recompute via byte indices using char_indices
                let mut start_byte = offset;
                for (i, c) in before.char_indices().rev() {
                    if is_word_char(c) {
                        start_byte = i;
                    } else {
                        break;
                    }
                }
                let mut end_byte = offset;
                for (i, c) in after.char_indices() {
                    if is_word_char(c) {
                        end_byte = offset + i + c.len_utf8();
                    } else {
                        break;
                    }
                }

                Range::new(
                    crate::types::offset_to_position(&text, start_byte),
                    crate::types::offset_to_position(&text, end_byte),
                )
            });

        Ok(Some(PrepareRenameResponse::Range(range)))
    }

    async fn rename(
        &self,
        params: RenameParams,
    ) -> tower_lsp_server::jsonrpc::Result<Option<WorkspaceEdit>> {
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
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();

        for def in definitions {
            let range = def.name_range.unwrap_or(def.range);
            changes.entry(def.uri.clone()).or_default().push(TextEdit {
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
    ) -> tower_lsp_server::jsonrpc::Result<Option<DocumentSymbolResponse>> {
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
    ) -> tower_lsp_server::jsonrpc::Result<Option<WorkspaceSymbolResponse>> {
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
                location: Location::new(var.uri.clone(), var.range),
                container_name: None,
            });
        }

        Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
    }
}

impl CssVariableLsp {
    async fn literal_color_under_cursor(
        &self,
        uri: &Uri,
        position: Position,
    ) -> Option<NormalizedColorKey> {
        let occurrences = self.manager.get_document_literal_colors(uri).await;
        occurrences
            .into_iter()
            .find(|occurrence| range_contains_position(&occurrence.range, position))
            .map(|occurrence| occurrence.normalized_color)
    }

    /// Scan workspace folders for CSS and HTML files
    pub async fn scan_workspace_folders(&self, folders: Vec<WorkspaceFolder>) {
        let folder_uris: Vec<Uri> = folders.iter().map(|f| f.uri.clone()).collect();

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

#[allow(clippy::too_many_arguments)]
async fn validate_document_text_with(
    client: &Client,
    manager: &CssVariableManager,
    usage_regex: &Regex,
    undefined_var_fallback: UndefinedVarFallbackMode,
    has_related_info: bool,
    uri: &Uri,
    text: &str,
    document_usage_map: &Arc<RwLock<HashMap<Uri, HashSet<String>>>>,
    usage_index: &Arc<RwLock<HashMap<String, HashSet<Uri>>>>,
) {
    let mut diagnostics = Vec::new();
    let mut current_usages = HashSet::new();
    let default_severity = DiagnosticSeverity::WARNING;

    for captures in usage_regex.captures_iter(text) {
        let match_all = captures.get(0).unwrap();
        let name = captures.get(1).unwrap().as_str();
        let fallback = captures.get(2).map(|m| m.as_str()).unwrap_or("");
        let has_fallback = !fallback.trim().is_empty();

        current_usages.insert(name.to_string());

        let definitions = manager.get_variables(name).await;
        if !definitions.is_empty() {
            continue;
        }

        let severity = if has_fallback {
            match undefined_var_fallback {
                UndefinedVarFallbackMode::Warning => Some(default_severity),
                UndefinedVarFallbackMode::Info => Some(DiagnosticSeverity::INFORMATION),
                UndefinedVarFallbackMode::Off => None,
            }
        } else {
            Some(default_severity)
        };
        let severity = match severity {
            Some(severity) => severity,
            None => continue,
        };
        let range = Range::new(
            crate::types::offset_to_position(text, match_all.start()),
            crate::types::offset_to_position(text, match_all.end()),
        );
        diagnostics.push(Diagnostic {
            range,
            severity: Some(severity),
            code: Some(ls_types::NumberOrString::String(
                "css-variable-lsp.undefined-variable".to_string(),
            )),
            code_description: None,
            source: Some("css-variable-lsp".to_string()),
            message: format!("CSS variable '{}' is not defined in the workspace", name),
            related_information: if has_related_info {
                Some(Vec::new())
            } else {
                None
            },
            tags: None,
            data: Some(serde_json::json!({
                "name": name,
                "hasFallback": has_fallback,
                "range": {
                    "start": { "line": range.start.line, "character": range.start.character },
                    "end": { "line": range.end.line, "character": range.end.character }
                }
            })),
        });
    }

    for occurrence in manager.get_document_literal_colors(uri).await {
        let replacements = manager
            .get_variables_by_color_key(&occurrence.normalized_color)
            .await;
        if replacements.is_empty() {
            continue;
        }

        let replacement_data: Vec<_> = replacements
            .iter()
            .map(|var| {
                serde_json::json!({
                    "name": var.name,
                    "value": var.value,
                    "uri": var.uri,
                })
            })
            .collect();
        let replacement_count = replacement_data.len();
        let message = if replacement_count == 1 {
            "A matching CSS variable exists for this literal color".to_string()
        } else {
            format!(
                "Matching CSS variables exist for this literal color ({} replacements)",
                replacement_count
            )
        };

        diagnostics.push(Diagnostic {
            range: occurrence.range,
            severity: Some(DiagnosticSeverity::INFORMATION),
            code: Some(ls_types::NumberOrString::String(
                "css-variable-lsp.literal-color-replaceable".to_string(),
            )),
            code_description: None,
            source: Some("css-variable-lsp".to_string()),
            message,
            related_information: None,
            tags: None,
            data: Some(serde_json::json!({
                "literal": occurrence.text,
                "usageContext": occurrence.usage_context,
                "replacements": replacement_data,
            })),
        });
    }

    // Update usage maps
    {
        let mut usage_map = document_usage_map.write().await;
        let old_usages = usage_map.insert(uri.clone(), current_usages.clone());

        let mut index = usage_index.write().await;

        // Remove old usages from index
        if let Some(old_set) = old_usages {
            for name in old_set {
                if !current_usages.contains(&name) {
                    if let Some(uris) = index.get_mut(&name) {
                        uris.remove(uri);
                        if uris.is_empty() {
                            index.remove(&name);
                        }
                    }
                }
            }
        }

        // Add new usages to index
        for name in current_usages {
            index
                .entry(name)
                .or_insert_with(HashSet::new)
                .insert(uri.clone());
        }
    }

    client
        .publish_diagnostics(uri.clone(), diagnostics, None)
        .await;
}

fn is_html_like_extension(ext: &str) -> bool {
    matches!(ext, ".html" | ".vue" | ".svelte" | ".astro" | ".ripple")
}

fn range_contains_position(range: &Range, position: Position) -> bool {
    range.start <= position && position <= range.end
}

fn language_id_kind(language_id: &str) -> Option<DocumentKind> {
    match language_id.to_lowercase().as_str() {
        "html" | "vue" | "svelte" | "astro" | "ripple" => Some(DocumentKind::Html),
        "css" | "scss" | "sass" | "less" => Some(DocumentKind::Css),
        _ => None,
    }
}

fn normalize_extension(ext: &str) -> Option<String> {
    let trimmed = ext.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(".{}", trimmed.to_lowercase()))
}

fn extract_extensions(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    if let (Some(start), Some(end)) = (pattern.find('{'), pattern.find('}')) {
        if end > start + 1 {
            let inner = &pattern[start + 1..end];
            return inner.split(',').filter_map(normalize_extension).collect();
        }
    }

    let ext = std::path::Path::new(pattern)
        .extension()
        .and_then(|ext| ext.to_str());
    ext.and_then(normalize_extension).into_iter().collect()
}

fn build_lookup_extension_map(lookup_files: &[String]) -> HashMap<String, DocumentKind> {
    let mut map = HashMap::new();
    for pattern in lookup_files {
        for ext in extract_extensions(pattern) {
            let kind = if is_html_like_extension(&ext) {
                DocumentKind::Html
            } else {
                DocumentKind::Css
            };
            map.insert(ext, kind);
        }
    }
    map
}

fn resolve_document_kind(
    path: &str,
    language_id: Option<&str>,
    lookup_extension_map: &HashMap<String, DocumentKind>,
) -> Option<DocumentKind> {
    if let Some(language_id) = language_id {
        if let Some(kind) = language_id_kind(language_id) {
            return Some(kind);
        }
    }

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(normalize_extension)?;

    lookup_extension_map.get(&ext).copied()
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

fn is_word_byte(b: u8) -> bool {
    is_word_char(b as char)
}

fn is_var_function_context_slice(before_cursor: &str) -> bool {
    let bytes = before_cursor.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let mut i = bytes.len();
    if is_word_byte(bytes[i - 1]) {
        let mut start = i;
        while start > 0 && is_word_byte(bytes[start - 1]) {
            start -= 1;
        }
        if i - start < 2 || bytes[start] != b'-' || bytes[start + 1] != b'-' {
            return false;
        }
        i = start;
    }

    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }

    if i == 0 || bytes[i - 1] != b'(' {
        return false;
    }

    let paren_idx = i - 1;
    if paren_idx < 3 {
        return false;
    }
    let start = paren_idx - 3;
    if !bytes[start..paren_idx].eq_ignore_ascii_case(b"var") {
        return false;
    }
    if start == 0 {
        return true;
    }
    !is_word_byte(bytes[start - 1])
}

struct CompletionContextSlice<'a> {
    slice: &'a str,
    allow_without_braces: bool,
}

struct ValueContext {
    is_value_context: bool,
    property_name: Option<String>,
}

fn completion_value_context_slice<'a>(
    text: &'a str,
    position: Position,
    language_id: Option<&str>,
    uri: &Uri,
    lookup_extension_map: &HashMap<String, DocumentKind>,
) -> Option<CompletionContextSlice<'a>> {
    let offset = position_to_offset(text, position)?;
    let start = clamp_to_char_boundary(text, offset.saturating_sub(400));
    let offset = clamp_to_char_boundary(text, offset);
    let before_cursor = &text[start..offset];

    if is_js_like_document(uri.path().as_str(), language_id) {
        let slice = find_js_string_segment(before_cursor)?;
        return Some(CompletionContextSlice {
            slice,
            allow_without_braces: true,
        });
    }

    match resolve_document_kind(uri.path().as_str(), language_id, lookup_extension_map) {
        Some(DocumentKind::Html) => find_html_style_context_slice(before_cursor),
        Some(DocumentKind::Css) => Some(CompletionContextSlice {
            slice: before_cursor,
            allow_without_braces: false,
        }),
        None => None,
    }
}

fn is_js_like_language_id(language_id: &str) -> bool {
    matches!(
        language_id.to_lowercase().as_str(),
        "javascript"
            | "javascriptreact"
            | "typescript"
            | "typescriptreact"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
    )
}

fn is_js_like_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".js" | ".jsx" | ".ts" | ".tsx" | ".mjs" | ".cjs" | ".mts" | ".cts"
    )
}

fn is_js_like_document(path: &str, language_id: Option<&str>) -> bool {
    if let Some(language_id) = language_id {
        if is_js_like_language_id(language_id) {
            return true;
        }
    }

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(normalize_extension);
    ext.as_deref().map(is_js_like_extension).unwrap_or(false)
}

fn find_html_style_attribute_slice(before_cursor: &str) -> Option<&str> {
    let lower = before_cursor.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut search_end = lower.len();

    while let Some(idx) = lower[..search_end].rfind("style") {
        if idx > 0 && is_word_byte(bytes[idx - 1]) {
            search_end = idx;
            continue;
        }
        let after_idx = idx + 5;
        if after_idx < bytes.len() && is_word_byte(bytes[after_idx]) {
            search_end = idx;
            continue;
        }

        let mut j = after_idx;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            search_end = idx;
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }

        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            search_end = idx;
            continue;
        }
        let value_start = j + 1;
        let rest = &bytes[value_start..];
        if !rest.contains(&quote) {
            return Some(&before_cursor[value_start..]);
        }

        search_end = idx;
    }

    None
}

fn find_html_style_block_slice(before_cursor: &str) -> Option<&str> {
    let lower = before_cursor.to_ascii_lowercase();
    let open_idx = lower.rfind("<style")?;
    if let Some(close_idx) = lower.rfind("</style") {
        if close_idx > open_idx {
            return None;
        }
    }

    let tag_end_rel = lower[open_idx..].find('>')?;
    let tag_end = open_idx + tag_end_rel;
    if tag_end + 1 > before_cursor.len() {
        return None;
    }

    Some(&before_cursor[tag_end + 1..])
}

fn find_html_style_context_slice(before_cursor: &str) -> Option<CompletionContextSlice<'_>> {
    if let Some(slice) = find_html_style_attribute_slice(before_cursor) {
        return Some(CompletionContextSlice {
            slice,
            allow_without_braces: true,
        });
    }
    if let Some(slice) = find_html_style_block_slice(before_cursor) {
        return Some(CompletionContextSlice {
            slice,
            allow_without_braces: false,
        });
    }
    None
}

fn find_js_string_segment(before_cursor: &str) -> Option<&str> {
    let bytes = before_cursor.as_bytes();
    let mut in_quote: Option<u8> = None;
    let mut in_template = false;
    let mut template_expr_depth: i32 = 0;
    let mut expr_quote: Option<u8> = None;
    let mut segment_start: Option<usize> = None;

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_quote {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == q {
                in_quote = None;
                segment_start = None;
            }
            i += 1;
            continue;
        }

        if in_template {
            if template_expr_depth > 0 {
                if let Some(q) = expr_quote {
                    if b == b'\\' {
                        i = i.saturating_add(2);
                        continue;
                    }
                    if b == q {
                        expr_quote = None;
                    }
                    i += 1;
                    continue;
                }

                if b == b'\'' || b == b'"' || b == b'`' {
                    expr_quote = Some(b);
                    i += 1;
                    continue;
                }
                if b == b'{' {
                    template_expr_depth += 1;
                } else if b == b'}' {
                    template_expr_depth -= 1;
                    if template_expr_depth == 0 {
                        segment_start = Some(i + 1);
                    }
                }
                i += 1;
                continue;
            }

            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'`' {
                in_template = false;
                segment_start = None;
                i += 1;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                template_expr_depth = 1;
                segment_start = None;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if b == b'\'' || b == b'"' {
            in_quote = Some(b);
            segment_start = Some(i + 1);
            i += 1;
            continue;
        }
        if b == b'`' {
            in_template = true;
            segment_start = Some(i + 1);
            i += 1;
            continue;
        }
        i += 1;
    }

    if in_quote.is_some() {
        return segment_start.map(|start| &before_cursor[start..]);
    }
    if in_template && template_expr_depth == 0 {
        return segment_start.map(|start| &before_cursor[start..]);
    }
    None
}

fn find_context_colon(before_cursor: &str, allow_without_braces: bool) -> Option<usize> {
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
                    in_parens = 0;
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

    if !allow_without_braces && last_brace == -1 {
        return None;
    }

    if last_colon > last_semicolon && last_colon > last_brace {
        Some(last_colon as usize)
    } else {
        None
    }
}

fn get_value_context_info(before_cursor: &str, allow_without_braces: bool) -> ValueContext {
    let colon_pos = match find_context_colon(before_cursor, allow_without_braces) {
        Some(pos) => pos,
        None => {
            return ValueContext {
                is_value_context: false,
                property_name: None,
            }
        }
    };
    let before_colon = before_cursor[..colon_pos].trim_end();
    if before_colon.is_empty() {
        return ValueContext {
            is_value_context: true,
            property_name: None,
        };
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
        return ValueContext {
            is_value_context: true,
            property_name: None,
        };
    }

    ValueContext {
        is_value_context: true,
        property_name: Some(before_colon[start..].to_lowercase()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn test_word_extraction(css: &str, cursor_pos: usize) -> Option<String> {
        use ls_types::Position;
        let position = Position {
            line: 0,
            character: cursor_pos as u32,
        };

        let offset = position_to_offset(css, position)?;
        let offset = clamp_to_char_boundary(css, offset);
        let before = &css[..offset];
        let after = &css[offset..];

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

    #[test]
    fn test_word_extraction_preserves_fallbacks() {
        // Test extraction of variable name from var() call with fallback
        let css = "background: var(--primary-color, blue);";
        let result = test_word_extraction(css, 20); // cursor on 'p' in --primary-color
        assert_eq!(result, Some("--primary-color".to_string()));

        // Test that fallback is not included
        let css2 = "color: var(--secondary-color, #ccc);";
        let result2 = test_word_extraction(css2, 15); // cursor on 's' in --secondary-color
        assert_eq!(result2, Some("--secondary-color".to_string()));

        // Test nested fallback - should still extract the main variable
        let css3 = "border: var(--accent-color, var(--fallback, black));";
        let result3 = test_word_extraction(css3, 16); // cursor on 'a' in --accent-color
        assert_eq!(result3, Some("--accent-color".to_string()));

        // Test simple variable without var()
        let css4 = "--theme-color: red;";
        let result4 = test_word_extraction(css4, 5); // cursor on 't' in --theme-color
        assert_eq!(result4, Some("--theme-color".to_string()));

        // Test variable at end of line
        let css5 = "margin: var(--spacing);";
        let result5 = test_word_extraction(css5, 15); // cursor on 's' in --spacing
        assert_eq!(result5, Some("--spacing".to_string()));
    }

    #[test]
    fn test_var_function_context_open() {
        let text = "color: var(--primary";
        assert!(is_var_function_context_slice(text));
    }

    #[test]
    fn test_var_function_context_closed() {
        let text = "color: var(--primary);";
        assert!(!is_var_function_context_slice(text));
    }

    #[test]
    fn test_var_function_context_nested() {
        let text = "color: var(--primary, calc(100% - var(--secondary";
        assert!(is_var_function_context_slice(text));
    }

    #[test]
    fn test_var_function_context_after_fallback() {
        let text = "color: var(--primary, ";
        assert!(!is_var_function_context_slice(text));
    }

    #[test]
    fn test_var_function_context_requires_boundary() {
        let text = "navbar(--primary";
        assert!(!is_var_function_context_slice(text));
    }

    #[test]
    fn test_var_function_context_case_insensitive() {
        let text = "color: VAR(--primary";
        assert!(is_var_function_context_slice(text));
    }

    #[test]
    fn test_completion_value_context_slice_css() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = ".card { color: var(";
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///styles.css").unwrap();
        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected css slice");
        assert_eq!(context.slice, text);
        assert!(!context.allow_without_braces);
    }

    #[test]
    fn test_completion_value_context_slice_html_style_attribute() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = r#"<div style="color: var("#;
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///index.html").unwrap();
        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected html style attribute slice");
        assert_eq!(context.slice, "color: var(");
        assert!(context.allow_without_braces);
    }

    #[test]
    fn test_completion_value_context_slice_html_style_block() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = "<style>body { color: var(";
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///index.html").unwrap();
        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected html style block slice");
        assert_eq!(context.slice, "body { color: var(");
        assert!(!context.allow_without_braces);
    }

    #[test]
    fn test_completion_value_context_slice_js_string() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = r#"const css = "color: var("#;
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///app.js").unwrap();
        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected js string slice");
        assert_eq!(context.slice, "color: var(");
        assert!(context.allow_without_braces);
    }

    #[test]
    fn test_completion_value_context_slice_js_non_string() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = "const css = color: var(";
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///app.js").unwrap();
        assert!(completion_value_context_slice(text, position, None, &uri, &lookup_map).is_none());
    }

    #[test]
    fn test_completion_value_context_slice_unknown() {
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let text = "color: var(";
        let position = crate::types::offset_to_position(text, text.len());
        let uri = Uri::from_str("file:///notes.txt").unwrap();
        assert!(completion_value_context_slice(text, position, None, &uri, &lookup_map).is_none());
    }

    #[test]
    fn test_html_style_attribute_slice_open() {
        let text = r#"<div style="color: var("#;
        let slice = find_html_style_attribute_slice(text).unwrap();
        assert_eq!(slice, "color: var(");
    }

    #[test]
    fn test_html_style_attribute_slice_closed() {
        let text = r#"<div style="color: red">"#;
        assert!(find_html_style_attribute_slice(text).is_none());
    }

    #[test]
    fn test_html_style_block_slice() {
        let text = "<style>body { color: var(";
        let slice = find_html_style_block_slice(text).unwrap();
        assert_eq!(slice, "body { color: var(");
    }

    #[test]
    fn test_js_string_segment_basic() {
        let text = r#"const css = \"color: var("#;
        let slice = find_js_string_segment(text).unwrap();
        assert_eq!(slice, "color: var(");
    }

    #[test]
    fn test_js_string_segment_template_after_expression() {
        let text = r#"const css = `color: ${theme}; background: var("#;
        let slice = find_js_string_segment(text).unwrap();
        assert_eq!(slice, "; background: var(");
    }

    #[test]
    fn test_js_string_segment_template_expression() {
        let text = r#"const css = `color: ${theme"#;
        assert!(find_js_string_segment(text).is_none());
    }

    #[test]
    fn resolve_document_kind_prefers_language_id() {
        let lookup_files = vec!["**/*.custom".to_string()];
        let lookup_map = build_lookup_extension_map(&lookup_files);

        let kind = resolve_document_kind("file.custom", Some("html"), &lookup_map);
        assert_eq!(kind, Some(DocumentKind::Html));
    }

    #[test]
    fn resolve_document_kind_uses_lookup_extensions() {
        let lookup_files = vec![
            "**/*.{css,scss}".to_string(),
            "**/*.vue".to_string(),
            "**/*.custom".to_string(),
        ];
        let lookup_map = build_lookup_extension_map(&lookup_files);

        let css_kind = resolve_document_kind("styles.scss", None, &lookup_map);
        assert_eq!(css_kind, Some(DocumentKind::Css));

        let html_kind = resolve_document_kind("component.vue", None, &lookup_map);
        assert_eq!(html_kind, Some(DocumentKind::Html));

        let custom_kind = resolve_document_kind("theme.custom", None, &lookup_map);
        assert_eq!(custom_kind, Some(DocumentKind::Css));
    }

    #[test]
    fn resolve_document_kind_returns_none_for_unknown() {
        let lookup_files = vec!["**/*.css".to_string()];
        let lookup_map = build_lookup_extension_map(&lookup_files);

        let kind = resolve_document_kind("notes.txt", Some("plaintext"), &lookup_map);
        assert_eq!(kind, None);
    }
}
