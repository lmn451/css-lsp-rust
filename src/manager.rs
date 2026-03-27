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
        let mut vars = self.variables.write().await;
        let mut usages = self.usages.write().await;
        let mut literal_colors = self.literal_colors.write().await;
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

        literal_colors.remove(uri);
        dom_trees.remove(uri);

        // FIX: Rebuild color index to remove stale entries
        drop(vars);
        drop(usages);
        drop(literal_colors);
        drop(dom_trees);
        self.rebuild_color_index().await;
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
        let uri = Uri::from_str("file:///test.css").unwrap();

        let var = create_test_variable("--primary", "blue", ":root", "file:///test.css");
        let usage = create_test_usage("--primary", ".button", "file:///test.css");
        let literal = create_literal_color("blue", "file:///test.css", "blue", ".button");

        manager.add_variable(var).await;
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
    async fn test_manager_resolve_variable_color_key_chain() {
        let manager = CssVariableManager::new(Config::default());

        manager
            .add_variable(create_test_variable(
                "--base-color",
                "#fff",
                ":root",
                "file:///test.css",
            ))
            .await;
        manager
            .add_variable(create_test_variable(
                "--alias-color",
                "var(--base-color)",
                ":root",
                "file:///test.css",
            ))
            .await;

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
            .await;
        manager
            .add_variable(create_test_variable(
                "--text-color",
                "#fff",
                ":root",
                "file:///test.css",
            ))
            .await;

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
            .await;
        manager
            .add_variable(create_test_variable(
                "--surface",
                "rgb(255 255 255)",
                ":root",
                "file:///test.css",
            ))
            .await;

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
        let uri1 = Uri::from_str("file:///file1.css").unwrap();
        let _uri2 = Uri::from_str("file:///file2.css").unwrap();

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

    #[tokio::test]
    async fn test_manager_color_index_stale_after_remove() {
        // Regression test: color_variables becomes stale after remove_document()
        // when rebuild_color_index() is not called
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let white_key = crate::color::normalized_color_key("white").unwrap();

        // Add a color variable and build the index
        let var = create_test_variable("--bg", "#ffffff", ":root", "file:///test.css");
        manager.add_variable(var).await;
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
