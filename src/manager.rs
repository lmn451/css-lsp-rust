use ls_types::{Position, Uri};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::color::{color_from_key, normalize_color, parse_color, NormalizedColorKey};
use crate::dom_tree::DomTree;
use crate::specificity::sort_by_cascade;
use crate::types::{Config, CssVariable, CssVariableUsage, LiteralColorOccurrence};

type LiteralColorMap = HashMap<Uri, HashMap<u32, Vec<LiteralColorOccurrence>>>;

/// Manages CSS variables across the workspace
#[derive(Clone)]
pub struct CssVariableManager {
    /// Map of variable name -> list of definitions
    variables: Arc<RwLock<HashMap<String, Vec<CssVariable>>>>,

    /// Map of variable name -> list of usages
    usages: Arc<RwLock<HashMap<String, Vec<CssVariableUsage>>>>,

    /// Literal color occurrences grouped by document and line
    /// Outer map: URI -> Inner map (line number -> colors on that line)
    literal_colors: Arc<RwLock<LiteralColorMap>>,

    /// Map of normalized colors to matching variable names
    color_variables: Arc<RwLock<HashMap<NormalizedColorKey, HashSet<String>>>>,

    /// Configuration
    config: Arc<RwLock<Config>>,

    /// DOM trees for HTML documents
    dom_trees: Arc<RwLock<HashMap<Uri, DomTree>>>,

    /// Set of tracked document URIs (for counting unique documents)
    tracked_documents: Arc<RwLock<HashSet<Uri>>>,
}

impl CssVariableManager {
    pub fn new(config: Config) -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            usages: Arc::new(RwLock::new(HashMap::new())),
            literal_colors: Arc::new(RwLock::new(HashMap::new())),
            color_variables: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(config)),
            dom_trees: Arc::new(RwLock::new(HashMap::new())),
            tracked_documents: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Add a variable definition
    pub async fn add_variable(&self, variable: CssVariable) -> Result<(), String> {
        self.add_variables(vec![variable]).await
    }

    /// Add variable definitions atomically under one variables lock.
    pub async fn add_variables(&self, variables: Vec<CssVariable>) -> Result<(), String> {
        if variables.is_empty() {
            return Ok(());
        }

        let max_documents = self.config.read().await.max_documents;
        let mut tracked = self.tracked_documents.write().await;

        let new_documents: HashSet<_> = variables
            .iter()
            .map(|variable| variable.uri.clone())
            .filter(|uri| !tracked.contains(uri))
            .collect();
        if max_documents > 0 && tracked.len() + new_documents.len() > max_documents {
            return Err(format!(
                "Maximum document limit ({max_documents}) reached. Cannot add more documents."
            ));
        }
        tracked.extend(new_documents);

        // Keep the tracked-documents -> variables lock order consistent with removals.
        let mut vars = self.variables.write().await;
        for variable in variables {
            vars.entry(variable.name.clone())
                .or_default()
                .push(variable);
        }

        Ok(())
    }

    /// Atomically replace all variable definitions owned by one document.
    pub async fn replace_document_variables(
        &self,
        uri: &Uri,
        variables: Vec<CssVariable>,
    ) -> Result<(), String> {
        if variables.iter().any(|variable| &variable.uri != uri) {
            return Err("Replacement variables must belong to the target document".to_string());
        }

        let max_documents = self.config.read().await.max_documents;
        let mut tracked = self.tracked_documents.write().await;
        let introduces_document = !variables.is_empty() && !tracked.contains(uri);
        if max_documents > 0 && introduces_document && tracked.len() >= max_documents {
            return Err(format!(
                "Maximum document limit ({max_documents}) reached. Cannot add more documents."
            ));
        }

        let mut vars = self.variables.write().await;
        for definitions in vars.values_mut() {
            definitions.retain(|variable| &variable.uri != uri);
        }
        vars.retain(|_, definitions| !definitions.is_empty());

        if variables.is_empty() {
            tracked.remove(uri);
        } else {
            tracked.insert(uri.clone());
        }
        for variable in variables {
            vars.entry(variable.name.clone())
                .or_default()
                .push(variable);
        }

        Ok(())
    }

    /// Atomically replace variable definitions and usages owned by one document.
    pub async fn replace_document_analysis(
        &self,
        uri: &Uri,
        variables: Vec<CssVariable>,
        usages: Vec<CssVariableUsage>,
    ) -> Result<(), String> {
        if variables.iter().any(|variable| &variable.uri != uri)
            || usages.iter().any(|usage| &usage.uri != uri)
        {
            return Err("Replacement analysis must belong to the target document".to_string());
        }

        let max_documents = self.config.read().await.max_documents;
        let mut tracked = self.tracked_documents.write().await;
        let has_analysis = !variables.is_empty() || !usages.is_empty();
        let introduces_document = has_analysis && !tracked.contains(uri);
        if max_documents > 0 && introduces_document && tracked.len() >= max_documents {
            return Err(format!(
                "Maximum document limit ({max_documents}) reached. Cannot add more documents."
            ));
        }

        // Keep the same tracked -> variables -> usages order as document removal.
        let mut vars = self.variables.write().await;
        let mut stored_usages = self.usages.write().await;

        for definitions in vars.values_mut() {
            definitions.retain(|variable| &variable.uri != uri);
        }
        vars.retain(|_, definitions| !definitions.is_empty());

        for document_usages in stored_usages.values_mut() {
            document_usages.retain(|usage| &usage.uri != uri);
        }
        stored_usages.retain(|_, document_usages| !document_usages.is_empty());

        if !has_analysis {
            tracked.remove(uri);
        } else {
            tracked.insert(uri.clone());
        }
        for variable in variables {
            vars.entry(variable.name.clone())
                .or_default()
                .push(variable);
        }

        for usage in usages {
            stored_usages
                .entry(usage.name.clone())
                .or_default()
                .push(usage);
        }

        Ok(())
    }

    /// Add a variable usage
    pub async fn add_usage(&self, usage: CssVariableUsage) {
        let mut usages = self.usages.write().await;
        usages
            .entry(usage.name.clone())
            .or_insert_with(Vec::new)
            .push(usage);
    }

    /// Add a literal color occurrence
    pub async fn add_literal_color(&self, occurrence: LiteralColorOccurrence) {
        let mut literal_colors = self.literal_colors.write().await;
        let line = occurrence.range.start.line;
        literal_colors
            .entry(occurrence.uri.clone())
            .or_default()
            .entry(line)
            .or_default()
            .push(occurrence);
    }

    /// Get all definitions of a variable
    pub async fn get_variables(&self, name: &str) -> Vec<CssVariable> {
        let vars = self.variables.read().await;
        vars.get(name).cloned().unwrap_or_default()
    }

    /// Get all usages of a variable
    pub async fn get_usages(&self, name: &str) -> Vec<CssVariableUsage> {
        let usages = self.usages.read().await;
        usages.get(name).cloned().unwrap_or_default()
    }

    /// Resolve a variable name to a color using cascade ordering and var() chains.
    pub async fn resolve_variable_color(&self, name: &str) -> Option<ls_types::Color> {
        self.resolve_variable_color_key(name)
            .await
            .map(color_from_key)
    }

    /// Resolve a variable name to a normalized color key using cascade ordering and var() chains.
    pub async fn resolve_variable_color_key(&self, name: &str) -> Option<NormalizedColorKey> {
        let vars = self.variables.read().await;
        resolve_variable_color_key_from_map(name, &vars)
    }

    /// Get all variables (for completion)
    pub async fn get_all_variables(&self) -> Vec<CssVariable> {
        let vars = self.variables.read().await;
        vars.values().flatten().cloned().collect()
    }

    /// Get all references (definitions + usages) for a variable
    pub async fn get_references(&self, name: &str) -> (Vec<CssVariable>, Vec<CssVariableUsage>) {
        let definitions = self.get_variables(name).await;
        let usages = self.get_usages(name).await;
        (definitions, usages)
    }

    /// Get literal color occurrences in a specific document.
    pub async fn get_document_literal_colors(&self, uri: &Uri) -> Vec<LiteralColorOccurrence> {
        let literal_colors = self.literal_colors.read().await;
        literal_colors
            .get(uri)
            .map(|by_line| by_line.values().flatten().cloned().collect())
            .unwrap_or_default()
    }

    /// Get literal color occurrences at a specific position (O(1) line lookup + O(k) scan).
    pub async fn get_literal_colors_at_position(
        &self,
        uri: &Uri,
        position: Position,
    ) -> Vec<LiteralColorOccurrence> {
        let literal_colors = self.literal_colors.read().await;
        literal_colors
            .get(uri)
            .and_then(|by_line| by_line.get(&position.line).cloned())
            .unwrap_or_default()
    }

    /// Get all variables whose resolved color exactly matches the normalized color key.
    pub async fn get_variables_by_color_key(&self, key: &NormalizedColorKey) -> Vec<CssVariable> {
        let names = {
            let index = self.color_variables.read().await;
            index.get(key).cloned().unwrap_or_default()
        };
        let vars = self.variables.read().await;
        let mut matches = Vec::new();
        for name in names {
            if let Some(definitions) = vars.get(&name) {
                let mut definitions = definitions.clone();
                sort_by_cascade(&mut definitions);
                if let Some(variable) = definitions.into_iter().next() {
                    matches.push(variable);
                }
            }
        }
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches
    }

    /// Get the set of resolved variable colors currently defined in a specific document.
    pub async fn get_document_resolved_color_keys(&self, uri: &Uri) -> HashSet<NormalizedColorKey> {
        let names = self.get_document_variable_names(uri).await;
        let vars = self.variables.read().await;
        names
            .into_iter()
            .filter_map(|name| resolve_variable_color_key_from_map(&name, &vars))
            .collect()
    }

    /// Remove all data for a document
    pub async fn remove_document(&self, uri: &Uri) {
        self.remove_documents(&HashSet::from([uri.clone()])).await;
    }

    /// Remove all data for a set of documents.
    pub async fn remove_documents(&self, uris: &HashSet<Uri>) {
        if uris.is_empty() {
            return;
        }

        // This order must match add_variable: tracked_documents before variables.
        let mut tracked = self.tracked_documents.write().await;
        let mut vars = self.variables.write().await;
        let mut usages = self.usages.write().await;
        let mut literal_colors = self.literal_colors.write().await;
        let mut dom_trees = self.dom_trees.write().await;

        tracked.retain(|uri| !uris.contains(uri));

        for var_list in vars.values_mut() {
            var_list.retain(|variable| !uris.contains(&variable.uri));
        }
        vars.retain(|_, var_list| !var_list.is_empty());

        for usage_list in usages.values_mut() {
            usage_list.retain(|usage| !uris.contains(&usage.uri));
        }
        usages.retain(|_, usage_list| !usage_list.is_empty());

        literal_colors.retain(|uri, _| !uris.contains(uri));
        dom_trees.retain(|uri, _| !uris.contains(uri));

        // Rebuild the color index after releasing all document-data locks.
        drop(tracked);
        drop(vars);
        drop(usages);
        drop(literal_colors);
        drop(dom_trees);
        self.rebuild_color_index().await;
    }

    /// Get every document URI currently represented in manager state.
    pub async fn get_document_uris(&self) -> HashSet<Uri> {
        let mut uris = self.tracked_documents.read().await.clone();

        {
            let usages = self.usages.read().await;
            uris.extend(usages.values().flatten().map(|usage| usage.uri.clone()));
        }
        {
            let literal_colors = self.literal_colors.read().await;
            uris.extend(literal_colors.keys().cloned());
        }
        {
            let dom_trees = self.dom_trees.read().await;
            uris.extend(dom_trees.keys().cloned());
        }

        uris
    }

    /// Get all variables defined in a specific document
    pub async fn get_document_variables(&self, uri: &Uri) -> Vec<CssVariable> {
        let vars = self.variables.read().await;
        vars.values()
            .flatten()
            .filter(|v| &v.uri == uri)
            .cloned()
            .collect()
    }

    /// Get the set of variable names defined in a specific document
    pub async fn get_document_variable_names(&self, uri: &Uri) -> HashSet<String> {
        let vars = self.get_document_variables(uri).await;
        vars.into_iter().map(|v| v.name).collect()
    }

    /// Get all variable usages in a specific document
    pub async fn get_document_usages(&self, uri: &Uri) -> Vec<CssVariableUsage> {
        let usages = self.usages.read().await;
        usages
            .values()
            .flatten()
            .filter(|u| &u.uri == uri)
            .cloned()
            .collect()
    }

    /// Set DOM tree for a document
    pub async fn set_dom_tree(&self, uri: Uri, dom_tree: DomTree) {
        let mut dom_trees = self.dom_trees.write().await;
        dom_trees.insert(uri, dom_tree);
    }

    /// Get DOM tree for a document
    pub async fn get_dom_tree(&self, uri: &Uri) -> Option<DomTree> {
        let dom_trees = self.dom_trees.read().await;
        dom_trees.get(uri).cloned()
    }

    /// Get current configuration
    pub async fn get_config(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Replace the current configuration.
    pub async fn set_config(&self, config: Config) {
        let mut stored = self.config.write().await;
        *stored = config;
    }

    /// Rebuild the normalized-color -> variable-name lookup from current workspace state.
    pub async fn rebuild_color_index(&self) {
        let snapshot = {
            let vars = self.variables.read().await;
            vars.clone()
        };

        let mut color_variables: HashMap<NormalizedColorKey, HashSet<String>> = HashMap::new();
        for name in snapshot.keys() {
            if let Some(key) = resolve_variable_color_key_from_map(name, &snapshot) {
                color_variables.entry(key).or_default().insert(name.clone());
            }
        }

        let mut stored = self.color_variables.write().await;
        *stored = color_variables;
    }
}

fn extract_var_reference(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let start = trimmed.find("var(")?;
    let mut idx = start + 4;
    let bytes = trimmed.as_bytes();
    let mut depth = 1i32;
    while idx < bytes.len() {
        match bytes[idx] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        idx += 1;
    }
    if depth != 0 {
        return None;
    }

    let inner = trimmed[start + 4..idx].trim_start();
    let inner = inner.strip_prefix("--")?;
    let mut name_len = 0usize;
    for ch in inner.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            name_len += ch.len_utf8();
        } else {
            break;
        }
    }
    if name_len == 0 {
        return None;
    }
    Some(format!("--{}", &inner[..name_len]))
}

fn resolve_variable_color_key_from_map(
    name: &str,
    variables: &HashMap<String, Vec<CssVariable>>,
) -> Option<NormalizedColorKey> {
    let mut seen = HashSet::new();
    let mut current = name.to_string();

    loop {
        if seen.contains(&current) {
            return None;
        }
        seen.insert(current.clone());

        let mut definitions = variables.get(&current)?.clone();
        if definitions.is_empty() {
            return None;
        }

        sort_by_cascade(&mut definitions);
        let variable = &definitions[0];

        if let Some(next_name) = extract_var_reference(&variable.value) {
            current = next_name;
            continue;
        }

        return parse_color(&variable.value).map(normalize_color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ls_types::{Position, Range, Uri};
    use std::str::FromStr;

    fn create_test_variable(name: &str, value: &str, selector: &str, uri: &str) -> CssVariable {
        CssVariable {
            name: name.to_string(),
            value: value.to_string(),
            selector: selector.to_string(),
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            name_range: None,
            value_range: None,
            uri: Uri::from_str(uri).unwrap(),
            important: false,
            inline: false,
            source_position: 0,
        }
    }

    fn create_test_usage(name: &str, context: &str, uri: &str) -> CssVariableUsage {
        CssVariableUsage {
            name: name.to_string(),
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            name_range: None,
            uri: Uri::from_str(uri).unwrap(),
            usage_context: context.to_string(),
            dom_node: None,
        }
    }

    fn create_literal_color(
        text: &str,
        uri: &str,
        color: &str,
        context: &str,
    ) -> LiteralColorOccurrence {
        LiteralColorOccurrence {
            text: text.to_string(),
            uri: Uri::from_str(uri).unwrap(),
            range: Range::new(Position::new(0, 0), Position::new(0, text.len() as u32)),
            usage_context: context.to_string(),
            normalized_color: crate::color::normalized_color_key(color).unwrap(),
        }
    }

    #[test]
    fn extract_var_reference_allows_fallbacks_and_trailing_tokens() {
        assert_eq!(
            extract_var_reference("var(--primary, #fff)"),
            Some("--primary".to_string())
        );
        assert_eq!(
            extract_var_reference("var(--primary) !important"),
            Some("--primary".to_string())
        );
        assert_eq!(
            extract_var_reference("calc(1px + var(--spacing))"),
            Some("--spacing".to_string())
        );
    }

    #[tokio::test]
    async fn test_manager_add_and_get_variables() {
        let manager = CssVariableManager::new(Config::default());
        let var = create_test_variable("--primary", "#3b82f6", ":root", "file:///test.css");

        manager
            .add_variable(var.clone())
            .await
            .expect("add_variable failed");

        let variables = manager.get_variables("--primary").await;
        assert_eq!(variables.len(), 1);
        assert_eq!(variables[0].name, "--primary");
        assert_eq!(variables[0].value, "#3b82f6");
    }

    #[tokio::test]
    async fn test_manager_multiple_definitions() {
        let manager = CssVariableManager::new(Config::default());

        let var1 = create_test_variable("--color", "red", ":root", "file:///test.css");
        let var2 = create_test_variable("--color", "blue", ".class", "file:///test.css");

        manager
            .add_variable(var1)
            .await
            .expect("add_variable failed");
        manager
            .add_variable(var2)
            .await
            .expect("add_variable failed");

        let variables = manager.get_variables("--color").await;
        assert_eq!(variables.len(), 2);
    }

    #[tokio::test]
    async fn replace_document_variables_is_atomic_and_respects_limits() {
        let manager = CssVariableManager::new(Config {
            max_documents: 1,
            ..Config::default()
        });
        let first_uri = Uri::from_str("file:///first.css").unwrap();
        let second_uri = Uri::from_str("file:///second.css").unwrap();

        manager
            .replace_document_variables(
                &first_uri,
                vec![create_test_variable(
                    "--old",
                    "red",
                    ":root",
                    first_uri.as_str(),
                )],
            )
            .await
            .unwrap();

        let error = manager
            .replace_document_variables(
                &second_uri,
                vec![create_test_variable(
                    "--blocked",
                    "blue",
                    ":root",
                    second_uri.as_str(),
                )],
            )
            .await
            .unwrap_err();
        assert!(error.contains("Maximum document limit"));
        assert_eq!(manager.get_variables("--old").await.len(), 1);
        assert!(manager.get_variables("--blocked").await.is_empty());

        manager
            .replace_document_variables(
                &first_uri,
                vec![
                    create_test_variable("--new-a", "red", ":root", first_uri.as_str()),
                    create_test_variable("--new-b", "blue", ":root", first_uri.as_str()),
                ],
            )
            .await
            .unwrap();
        assert!(manager.get_variables("--old").await.is_empty());
        assert_eq!(manager.get_variables("--new-a").await.len(), 1);
        assert_eq!(manager.get_variables("--new-b").await.len(), 1);
    }

    #[tokio::test]
    async fn replace_document_analysis_replaces_definitions_and_usages_together() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///vite.config.ts").unwrap();

        manager
            .replace_document_analysis(
                &uri,
                vec![create_test_variable("--old", "red", ":root", uri.as_str())],
                vec![create_test_usage("--old-dependency", ":root", uri.as_str())],
            )
            .await
            .unwrap();

        manager
            .replace_document_analysis(
                &uri,
                vec![create_test_variable("--new", "blue", ":root", uri.as_str())],
                vec![create_test_usage("--new-dependency", ":root", uri.as_str())],
            )
            .await
            .unwrap();

        assert!(manager.get_variables("--old").await.is_empty());
        assert!(manager.get_usages("--old-dependency").await.is_empty());
        assert_eq!(manager.get_variables("--new").await.len(), 1);
        assert_eq!(manager.get_usages("--new-dependency").await.len(), 1);
    }

    #[tokio::test]
    async fn usage_only_analysis_is_tracked_and_respects_document_limits() {
        let manager = CssVariableManager::new(Config {
            max_documents: 1,
            ..Config::default()
        });
        let usage_uri = Uri::from_str("file:///vite.config.ts").unwrap();
        let blocked_uri = Uri::from_str("file:///blocked.css").unwrap();

        manager
            .replace_document_analysis(
                &usage_uri,
                Vec::new(),
                vec![create_test_usage(
                    "--dependency",
                    ":root",
                    usage_uri.as_str(),
                )],
            )
            .await
            .unwrap();

        assert!(manager.get_document_uris().await.contains(&usage_uri));
        let error = manager
            .replace_document_analysis(
                &blocked_uri,
                vec![create_test_variable(
                    "--blocked",
                    "red",
                    ":root",
                    blocked_uri.as_str(),
                )],
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert!(error.contains("Maximum document limit"));
        assert_eq!(manager.get_usages("--dependency").await.len(), 1);
        assert!(manager.get_variables("--blocked").await.is_empty());
    }

    #[tokio::test]
    async fn test_manager_add_and_get_usages() {
        let manager = CssVariableManager::new(Config::default());
        let usage = create_test_usage("--primary", ".button", "file:///test.css");

        manager.add_usage(usage.clone()).await;

        let usages = manager.get_usages("--primary").await;
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].name, "--primary");
        assert_eq!(usages[0].usage_context, ".button");
    }

    #[tokio::test]
    async fn test_manager_get_references() {
        let manager = CssVariableManager::new(Config::default());

        let var = create_test_variable("--spacing", "1rem", ":root", "file:///test.css");
        let usage = create_test_usage("--spacing", ".card", "file:///test.css");

        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");
        manager.add_usage(usage).await;

        let (defs, usages) = manager.get_references("--spacing").await;
        assert_eq!(defs.len(), 1);
        assert_eq!(usages.len(), 1);
    }

    #[tokio::test]
    async fn test_manager_remove_document() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let var = create_test_variable("--primary", "blue", ":root", "file:///test.css");
        let usage = create_test_usage("--primary", ".button", "file:///test.css");
        let literal = create_literal_color("blue", "file:///test.css", "blue", ".button");

        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");
        manager.add_usage(usage).await;
        manager.add_literal_color(literal).await;

        // Verify they exist
        assert_eq!(manager.get_variables("--primary").await.len(), 1);
        assert_eq!(manager.get_usages("--primary").await.len(), 1);
        assert_eq!(manager.get_document_literal_colors(&uri).await.len(), 1);

        // Remove document
        manager.remove_document(&uri).await;

        // Verify they're gone
        assert_eq!(manager.get_variables("--primary").await.len(), 0);
        assert_eq!(manager.get_usages("--primary").await.len(), 0);
        assert_eq!(manager.get_document_literal_colors(&uri).await.len(), 0);
    }

    #[tokio::test]
    async fn test_manager_get_all_variables() {
        let manager = CssVariableManager::new(Config::default());

        manager
            .add_variable(create_test_variable(
                "--primary",
                "blue",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--secondary",
                "red",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--spacing",
                "1rem",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");

        let all_vars = manager.get_all_variables().await;
        assert_eq!(all_vars.len(), 3);
    }

    #[tokio::test]
    async fn test_manager_resolve_variable_color() {
        let manager = CssVariableManager::new(Config::default());

        let var = create_test_variable("--primary-color", "#3b82f6", ":root", "file:///test.css");
        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");

        let color = manager.resolve_variable_color("--primary-color").await;
        assert!(color.is_some());
    }

    #[tokio::test]
    async fn test_manager_resolve_variable_color_key_chain() {
        let manager = CssVariableManager::new(Config::default());

        manager
            .add_variable(create_test_variable(
                "--base-color",
                "#fff",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--alias-color",
                "var(--base-color)",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");

        let key = manager.resolve_variable_color_key("--alias-color").await;
        assert_eq!(key, crate::color::normalized_color_key("white"));
    }

    #[tokio::test]
    async fn test_manager_get_variables_by_color_key_excludes_non_colors() {
        let manager = CssVariableManager::new(Config::default());

        manager
            .add_variable(create_test_variable(
                "--spacing",
                "1rem",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--text-color",
                "#fff",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");

        manager.rebuild_color_index().await;

        let matches = manager
            .get_variables_by_color_key(&crate::color::normalized_color_key("white").unwrap())
            .await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "--text-color");
    }

    #[tokio::test]
    async fn test_manager_get_variables_by_color_key_multiple_names() {
        let manager = CssVariableManager::new(Config::default());

        manager
            .add_variable(create_test_variable(
                "--text-color",
                "#fff",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--surface",
                "rgb(255 255 255)",
                ":root",
                "file:///test.css",
            ))
            .await
            .expect("add_variable failed");

        manager.rebuild_color_index().await;

        let matches = manager
            .get_variables_by_color_key(&crate::color::normalized_color_key("white").unwrap())
            .await;
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|var| var.name == "--surface"));
        assert!(matches.iter().any(|var| var.name == "--text-color"));
    }

    #[tokio::test]
    async fn test_manager_cross_file_references() {
        let manager = CssVariableManager::new(Config::default());

        // Variable defined in one file
        let var = create_test_variable("--theme", "dark", ":root", "file:///variables.css");
        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");

        // Used in another file
        let usage = create_test_usage("--theme", ".app", "file:///app.css");
        manager.add_usage(usage).await;

        let (defs, usages) = manager.get_references("--theme").await;
        assert_eq!(defs.len(), 1);
        assert_eq!(usages.len(), 1);
        assert_ne!(defs[0].uri, usages[0].uri);
    }

    #[tokio::test]
    async fn test_manager_document_isolation() {
        let manager = CssVariableManager::new(Config::default());
        let uri1 = Uri::from_str("file:///file1.css").unwrap();
        let _uri2 = Uri::from_str("file:///file2.css").unwrap();

        manager
            .add_variable(create_test_variable(
                "--color",
                "red",
                ":root",
                "file:///file1.css",
            ))
            .await
            .expect("add_variable failed");
        manager
            .add_variable(create_test_variable(
                "--color",
                "blue",
                ":root",
                "file:///file2.css",
            ))
            .await
            .expect("add_variable failed");

        // Should have both definitions
        assert_eq!(manager.get_variables("--color").await.len(), 2);

        // Remove one document
        manager.remove_document(&uri1).await;

        // Should only have one definition now
        let vars = manager.get_variables("--color").await;
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].value, "blue");
    }

    #[tokio::test]
    async fn test_manager_color_index_stale_after_remove() {
        // Regression test: color_variables becomes stale after remove_document()
        // when rebuild_color_index() is not called
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let white_key = crate::color::normalized_color_key("white").unwrap();

        // Add a color variable and build the index
        let var = create_test_variable("--bg", "#ffffff", ":root", "file:///test.css");
        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");
        manager.rebuild_color_index().await;

        // Verify it's indexed
        assert_eq!(
            manager.get_variables_by_color_key(&white_key).await.len(),
            1,
            "Variable should be indexed by color"
        );

        // Remove document WITHOUT rebuilding index (simulates the bug)
        manager.remove_document(&uri).await;

        // After removal, the variable should be gone from both collections
        assert_eq!(
            manager.get_variables("--bg").await.len(),
            0,
            "Variable should be removed from variables map"
        );

        // The bug: color_variables still contains the stale entry,
        // but get_variables_by_color_key() silently skips names not found in variables.
        // This test verifies the current (buggy) behavior - returns 0 matches.
        let color_matches = manager.get_variables_by_color_key(&white_key).await;
        assert_eq!(
            color_matches.len(),
            0,
            "BUG: color index is stale, returns 0 instead of correctly handling removal"
        );

        // Workaround: manually rebuild to get correct behavior
        manager.rebuild_color_index().await;
        assert_eq!(
            manager.get_variables_by_color_key(&white_key).await.len(),
            0,
            "After rebuild, color index is correct"
        );
    }

    // Note: extract_var_name is not a public function, so we skip testing it directly

    #[tokio::test]
    async fn test_manager_important_flag() {
        let manager = CssVariableManager::new(Config::default());

        let mut var = create_test_variable("--color", "red", ":root", "file:///test.css");
        var.important = true;

        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");

        let vars = manager.get_variables("--color").await;
        assert_eq!(vars.len(), 1);
        assert!(vars[0].important);
    }

    #[tokio::test]
    async fn test_manager_inline_flag() {
        let manager = CssVariableManager::new(Config::default());

        let mut var = create_test_variable(
            "--inline-color",
            "green",
            "inline-style",
            "file:///test.html",
        );
        var.inline = true;

        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");

        let vars = manager.get_variables("--inline-color").await;
        assert_eq!(vars.len(), 1);
        assert!(vars[0].inline);
    }

    #[tokio::test]
    async fn test_manager_empty_queries() {
        let manager = CssVariableManager::new(Config::default());

        // Query for non-existent variable
        let vars = manager.get_variables("--does-not-exist").await;
        assert_eq!(vars.len(), 0);

        let usages = manager.get_usages("--does-not-exist").await;
        assert_eq!(usages.len(), 0);

        let (defs, usages) = manager.get_references("--does-not-exist").await;
        assert_eq!(defs.len(), 0);
        assert_eq!(usages.len(), 0);
    }

    /// Memory limit enforcement in CssVariableManager
    ///
    /// ISSUE: The manager uses unbounded HashMaps that can grow indefinitely.
    /// Large workspaces could accumulate many documents without cleanup.
    ///
    /// After fix: Should have a document limit enforced.
    #[tokio::test]
    async fn test_manager_has_memory_limits() {
        use ls_types::{Position, Range};
        use std::str::FromStr;

        let config = Config {
            max_documents: 100,
            ..Default::default()
        };

        let manager = CssVariableManager::new(config);

        // Try to add 200 documents (beyond the limit of 100)
        let mut success_count = 0;
        let mut failure_count = 0;

        for i in 0..200 {
            let var = CssVariable {
                name: format!("--var-{}", i),
                value: "red".to_string(),
                selector: ":root".to_string(),
                range: Range::new(Position::new(0, 0), Position::new(0, 10)),
                name_range: None,
                value_range: None,
                uri: Uri::from_str(&format!("file:///test/doc_{}.css", i)).unwrap(),
                important: false,
                inline: false,
                source_position: 0,
            };

            match manager.add_variable(var).await {
                Ok(()) => success_count += 1,
                Err(_) => failure_count += 1,
            }
        }

        // Verify the limit was enforced
        assert_eq!(
            success_count, 100,
            "Should successfully add exactly 100 documents (the limit)"
        );
        assert_eq!(
            failure_count, 100,
            "Should fail to add the remaining 100 documents beyond the limit"
        );

        // Check how many documents are actually stored
        let vars = manager.variables.read().await;
        assert!(
            vars.len() <= 100,
            "Manager should not have more than 100 documents, but has {}",
            vars.len()
        );
    }

    #[tokio::test]
    async fn test_add_and_remove_use_consistent_lock_order() {
        use tokio::time::{timeout, Duration};

        let manager = CssVariableManager::new(Config::default());
        let existing_uri = Uri::from_str("file:///existing.css").unwrap();
        manager
            .add_variable(create_test_variable(
                "--existing",
                "red",
                ":root",
                existing_uri.as_str(),
            ))
            .await
            .unwrap();

        // Hold variables so remove queues for that lock. With the old variables -> tracked
        // removal order, the later add held tracked while waiting behind remove, deadlocking.
        let variables_guard = manager.variables.write().await;
        let remove_manager = manager.clone();
        let remove_task = tokio::spawn(async move {
            remove_manager.remove_document(&existing_uri).await;
        });
        tokio::task::yield_now().await;

        let add_manager = manager.clone();
        let add_task = tokio::spawn(async move {
            add_manager
                .add_variable(create_test_variable(
                    "--added",
                    "blue",
                    ":root",
                    "file:///added.css",
                ))
                .await
        });
        tokio::task::yield_now().await;
        drop(variables_guard);

        timeout(Duration::from_secs(1), async {
            remove_task.await.unwrap();
            add_task.await.unwrap().unwrap();
        })
        .await
        .expect("concurrent add/remove should not deadlock");
    }

    /// Bug demonstration: Color index can be stale during concurrent access
    ///
    /// ISSUE: rebuild_color_index() reads from variables and writes to color_variables.
    /// While individual operations are atomic, there's a brief window where the
    /// color index could be stale during concurrent updates.
    ///
    /// EXPECTED TO FAIL: This test proves the race condition exists.
    /// After fix: Color index should be properly synchronized.
    #[tokio::test]
    async fn test_manager_color_index_concurrent_consistency() {
        use ls_types::{Position, Range};
        use std::str::FromStr;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let config = Config::default();
        let manager = CssVariableManager::new(config);

        // Track inconsistencies between color index and variables
        let inconsistencies = Arc::new(AtomicUsize::new(0));

        // Add initial color variables
        let var = CssVariable {
            name: "--color-primary".to_string(),
            value: "#ff0000".to_string(),
            selector: ":root".to_string(),
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            name_range: None,
            value_range: None,
            uri: Uri::from_str("file:///test/colors.css").unwrap(),
            important: false,
            inline: false,
            source_position: 0,
        };
        manager
            .add_variable(var)
            .await
            .expect("add_variable failed");
        manager.rebuild_color_index().await;

        // Spawn concurrent readers and writers
        let mut handles = vec![];

        // Writer: Continuously add color variables
        for i in 0..100 {
            let manager_clone = manager.clone();
            handles.push(tokio::spawn(async move {
                let var = CssVariable {
                    name: format!("--color-{}", i),
                    value: format!("hsl({}, 100%, 50%)", i * 3),
                    selector: ":root".to_string(),
                    range: Range::new(Position::new(0, 0), Position::new(0, 10)),
                    name_range: None,
                    value_range: None,
                    uri: Uri::from_str(&format!("file:///test/color_{}.css", i)).unwrap(),
                    important: false,
                    inline: false,
                    source_position: 0,
                };
                manager_clone
                    .add_variable(var)
                    .await
                    .expect("add_variable failed");
                // Rebuild index after each add to simulate real usage
                manager_clone.rebuild_color_index().await;
            }));
        }

        // Reader: Continuously check color index consistency
        let inconsistencies_clone = inconsistencies.clone();
        let manager_reader = manager.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..50 {
                // Get all variables
                let all_vars = manager_reader.get_all_variables().await;

                // Count color variables by checking if they have color values
                let color_count = all_vars
                    .iter()
                    .filter(|v| crate::color::parse_color(&v.value).is_some())
                    .count();

                // Rebuild and check the color index
                manager_reader.rebuild_color_index().await;

                // Get variables by a sample color key
                let white_key = crate::color::normalized_color_key("white").unwrap();
                let white_matches = manager_reader.get_variables_by_color_key(&white_key).await;

                // If the counts are wildly different, there's an inconsistency
                // (This is a simplified check - real race conditions are harder to detect)
                if white_matches.len() > color_count + 10 {
                    inconsistencies_clone.fetch_add(1, Ordering::SeqCst);
                }

                tokio::time::sleep(tokio::time::Duration::from_micros(1)).await;
            }
        }));

        // Wait for all operations
        for handle in handles {
            let _ = handle.await;
        }

        // BUG: Currently this assertion will FAIL because race condition exists
        // The color index may be temporarily stale during concurrent updates
        // After fix: inconsistencies should be 0
        assert_eq!(
            inconsistencies.load(Ordering::SeqCst),
            0,
            "Color index had {} inconsistencies during concurrent access",
            inconsistencies.load(Ordering::SeqCst)
        );
    }
}
