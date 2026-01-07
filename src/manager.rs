use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::Url;

use crate::color::parse_color;
use crate::dom_tree::DomTree;
use crate::specificity::sort_by_cascade;
use crate::types::{Config, CssVariable, CssVariableUsage};

/// Manages CSS variables across the workspace
#[derive(Clone)]
pub struct CssVariableManager {
    /// Map of variable name -> list of definitions
    variables: Arc<RwLock<HashMap<String, Vec<CssVariable>>>>,

    /// Map of variable name -> list of usages
    usages: Arc<RwLock<HashMap<String, Vec<CssVariableUsage>>>>,

    /// Configuration
    config: Arc<RwLock<Config>>,

    /// DOM trees for HTML documents
    dom_trees: Arc<RwLock<HashMap<Url, DomTree>>>,
}

impl CssVariableManager {
    pub fn new(config: Config) -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            usages: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(config)),
            dom_trees: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a variable definition
    pub async fn add_variable(&self, variable: CssVariable) {
        let mut vars = self.variables.write().await;
        vars.entry(variable.name.clone())
            .or_insert_with(Vec::new)
            .push(variable);
    }

    /// Add a variable usage
    pub async fn add_usage(&self, usage: CssVariableUsage) {
        let mut usages = self.usages.write().await;
        usages
            .entry(usage.name.clone())
            .or_insert_with(Vec::new)
            .push(usage);
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
    pub async fn resolve_variable_color(&self, name: &str) -> Option<tower_lsp::lsp_types::Color> {
        let mut seen = std::collections::HashSet::new();
        let mut current = name.to_string();

        loop {
            if seen.contains(&current) {
                return None;
            }
            seen.insert(current.clone());

            let mut variables = self.get_variables(&current).await;
            if variables.is_empty() {
                return None;
            }

            sort_by_cascade(&mut variables);
            let variable = &variables[0];

            if let Some(next_name) = extract_var_reference(&variable.value) {
                current = next_name;
                continue;
            }

            return parse_color(&variable.value);
        }
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

    /// Remove all data for a document
    pub async fn remove_document(&self, uri: &Url) {
        let mut vars = self.variables.write().await;
        let mut usages = self.usages.write().await;
        let mut dom_trees = self.dom_trees.write().await;

        // Remove variables from this document
        for (_, var_list) in vars.iter_mut() {
            var_list.retain(|v| &v.uri != uri);
        }
        vars.retain(|_, var_list| !var_list.is_empty());

        // Remove usages from this document
        for (_, usage_list) in usages.iter_mut() {
            usage_list.retain(|u| &u.uri != uri);
        }
        usages.retain(|_, usage_list| !usage_list.is_empty());

        dom_trees.remove(uri);
    }

    /// Get all variables defined in a specific document
    pub async fn get_document_variables(&self, uri: &Url) -> Vec<CssVariable> {
        let vars = self.variables.read().await;
        vars.values()
            .flatten()
            .filter(|v| &v.uri == uri)
            .cloned()
            .collect()
    }

    /// Set DOM tree for a document
    pub async fn set_dom_tree(&self, uri: Url, dom_tree: DomTree) {
        let mut dom_trees = self.dom_trees.write().await;
        dom_trees.insert(uri, dom_tree);
    }

    /// Get DOM tree for a document
    pub async fn get_dom_tree(&self, uri: &Url) -> Option<DomTree> {
        let dom_trees = self.dom_trees.read().await;
        dom_trees.get(uri).cloned()
    }

    /// Get current configuration
    pub async fn get_config(&self) -> Config {
        self.config.read().await.clone()
    }
}

fn extract_var_reference(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.starts_with("var(") || !trimmed.ends_with(')') {
        return None;
    }
    let inner = trimmed.strip_prefix("var(")?.strip_suffix(')')?.trim();
    if inner.contains(',') || !inner.starts_with("--") {
        return None;
    }
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range, Url};

    fn create_test_variable(name: &str, value: &str, selector: &str, uri: &str) -> CssVariable {
        CssVariable {
            name: name.to_string(),
            value: value.to_string(),
            selector: selector.to_string(),
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            name_range: None,
            value_range: None,
            uri: Url::parse(uri).unwrap(),
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
            uri: Url::parse(uri).unwrap(),
            usage_context: context.to_string(),
            dom_node: None,
        }
    }

    #[tokio::test]
    async fn test_manager_add_and_get_variables() {
        let manager = CssVariableManager::new(Config::default());
        let var = create_test_variable("--primary", "#3b82f6", ":root", "file:///test.css");

        manager.add_variable(var.clone()).await;

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

        manager.add_variable(var1).await;
        manager.add_variable(var2).await;

        let variables = manager.get_variables("--color").await;
        assert_eq!(variables.len(), 2);
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

        manager.add_variable(var).await;
        manager.add_usage(usage).await;

        let (defs, usages) = manager.get_references("--spacing").await;
        assert_eq!(defs.len(), 1);
        assert_eq!(usages.len(), 1);
    }

    #[tokio::test]
    async fn test_manager_remove_document() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.css").unwrap();

        let var = create_test_variable("--primary", "blue", ":root", "file:///test.css");
        let usage = create_test_usage("--primary", ".button", "file:///test.css");

        manager.add_variable(var).await;
        manager.add_usage(usage).await;

        // Verify they exist
        assert_eq!(manager.get_variables("--primary").await.len(), 1);
        assert_eq!(manager.get_usages("--primary").await.len(), 1);

        // Remove document
        manager.remove_document(&uri).await;

        // Verify they're gone
        assert_eq!(manager.get_variables("--primary").await.len(), 0);
        assert_eq!(manager.get_usages("--primary").await.len(), 0);
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
            .await;
        manager
            .add_variable(create_test_variable(
                "--secondary",
                "red",
                ":root",
                "file:///test.css",
            ))
            .await;
        manager
            .add_variable(create_test_variable(
                "--spacing",
                "1rem",
                ":root",
                "file:///test.css",
            ))
            .await;

        let all_vars = manager.get_all_variables().await;
        assert_eq!(all_vars.len(), 3);
    }

    #[tokio::test]
    async fn test_manager_resolve_variable_color() {
        let manager = CssVariableManager::new(Config::default());

        let var = create_test_variable("--primary-color", "#3b82f6", ":root", "file:///test.css");
        manager.add_variable(var).await;

        let color = manager.resolve_variable_color("--primary-color").await;
        assert!(color.is_some());
    }

    #[tokio::test]
    async fn test_manager_cross_file_references() {
        let manager = CssVariableManager::new(Config::default());

        // Variable defined in one file
        let var = create_test_variable("--theme", "dark", ":root", "file:///variables.css");
        manager.add_variable(var).await;

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
        let uri1 = Url::parse("file:///file1.css").unwrap();
        let _uri2 = Url::parse("file:///file2.css").unwrap();

        manager
            .add_variable(create_test_variable(
                "--color",
                "red",
                ":root",
                "file:///file1.css",
            ))
            .await;
        manager
            .add_variable(create_test_variable(
                "--color",
                "blue",
                ":root",
                "file:///file2.css",
            ))
            .await;

        // Should have both definitions
        assert_eq!(manager.get_variables("--color").await.len(), 2);

        // Remove one document
        manager.remove_document(&uri1).await;

        // Should only have one definition now
        let vars = manager.get_variables("--color").await;
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].value, "blue");
    }

    // Note: extract_var_name is not a public function, so we skip testing it directly

    #[tokio::test]
    async fn test_manager_important_flag() {
        let manager = CssVariableManager::new(Config::default());

        let mut var = create_test_variable("--color", "red", ":root", "file:///test.css");
        var.important = true;

        manager.add_variable(var).await;

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

        manager.add_variable(var).await;

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
}
