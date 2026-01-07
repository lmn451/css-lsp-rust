use tower_lsp::lsp_types::Url;

use super::css::{parse_css_snippet, CssParseContext};
use crate::dom_tree::DomTree;
use crate::manager::CssVariableManager;
use crate::types::DOMNodeInfo;

/// Parse an HTML document and extract CSS from style blocks and inline styles
pub async fn parse_html_document(
    text: &str,
    uri: &Url,
    manager: &CssVariableManager,
) -> Result<(), String> {
    let parsed = DomTree::parse(text);

    manager
        .set_dom_tree(uri.clone(), parsed.dom_tree.clone())
        .await;

    for block in parsed.style_blocks {
        let context = CssParseContext {
            css_text: &block.content,
            full_text: text,
            uri,
            manager,
            base_offset: block.content_start,
            inline: false,
            usage_context_override: None,
            dom_node: None,
        };
        let _ = parse_css_snippet(context).await;
    }

    for inline in parsed.inline_styles {
        let dom_node: Option<DOMNodeInfo> = parsed
            .dom_tree
            .find_node_at_position(inline.attribute_start);

        let context = CssParseContext {
            css_text: &inline.value,
            full_text: text,
            uri,
            manager,
            base_offset: inline.value_start,
            inline: true,
            usage_context_override: Some("inline-style"),
            dom_node,
        };
        let _ = parse_css_snippet(context).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::CssVariableManager;
    use crate::types::Config;
    use std::collections::HashSet;

    #[tokio::test]
    async fn parse_html_document_extracts_definitions_and_usages() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();
        let text = r#"
            <style>
                :root { --primary: #fff; }
                .card { color: var(--primary); }
            </style>
            <div style="--accent: blue; color: var(--primary, #000)"></div>
        "#;

        parse_html_document(text, &uri, &manager).await.unwrap();

        let primary_defs = manager.get_variables("--primary").await;
        assert_eq!(primary_defs.len(), 1);
        assert_eq!(primary_defs[0].value, "#fff");

        let accent_defs = manager.get_variables("--accent").await;
        assert_eq!(accent_defs.len(), 1);
        assert_eq!(accent_defs[0].value, "blue");

        let usages = manager.get_usages("--primary").await;
        assert_eq!(usages.len(), 2);

        let contexts: HashSet<String> = usages.into_iter().map(|u| u.usage_context).collect();
        assert!(contexts.contains(".card"));
        assert!(contexts.contains("inline-style"));
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use crate::types::Config;
    use tower_lsp::lsp_types::Url;

    #[tokio::test]
    async fn test_parse_empty_html() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///empty.html").unwrap();

        let result = parse_html_document("", &uri, &manager).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parse_html_with_multiple_style_blocks() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <html>
                <head>
                    <style>:root { --head: blue; }</style>
                </head>
                <body>
                    <style>.body { --body: red; }</style>
                    <div>
                        <style>.nested { --nested: green; }</style>
                    </div>
                </body>
            </html>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 3);
    }

    #[tokio::test]
    async fn test_parse_html_with_inline_styles() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <div style="--inline1: red; color: var(--inline1);">
                <span style="--inline2: blue;">Test</span>
            </div>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);

        // Check that inline variables are marked as inline
        let inline_vars: Vec<_> = vars.iter().filter(|v| v.inline).collect();
        assert!(inline_vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_html_mixed_quotes() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <div style="--var1: blue;">Single</div>
            <div style='--var2: red;'>Double</div>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_html_with_comments() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <!-- HTML Comment -->
            <style>
                /* CSS Comment */
                :root { --color: blue; }
            </style>
            <div style="--inline: red;"><!-- Inline comment --></div>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_html_malformed() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        // Missing closing tags
        let html = r#"
            <div style="--var: blue;">
                <style>:root { --color: red; }
        "#;

        let result = parse_html_document(html, &uri, &manager).await;
        assert!(result.is_ok()); // Should still parse what it can
    }

    #[tokio::test]
    async fn test_parse_html_script_tags_ignored() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <html>
                <script>
                    const style = "--color: red;";
                </script>
                <style>:root { --real-color: blue; }</style>
            </html>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        // Should only find the real CSS variable, not the one in JavaScript
        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].name, "--real-color");
    }

    #[tokio::test]
    async fn test_parse_html_empty_style_attribute() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <div style="">Empty</div>
            <div style="  ">Whitespace</div>
        "#;

        let result = parse_html_document(html, &uri, &manager).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parse_html_style_with_media_queries() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <style>
                :root { --base: blue; }
                @media (min-width: 768px) {
                    :root { --responsive: red; }
                }
            </style>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_html_nested_elements_with_styles() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <div style="--outer: blue;">
                <div style="--middle: red;">
                    <div style="--inner: green;">
                        Content
                    </div>
                </div>
            </div>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 3);
    }

    #[tokio::test]
    async fn test_parse_html_special_characters_in_attributes() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.html").unwrap();

        let html = r#"
            <div 
                class="test-class" 
                data-value="some&value" 
                style="--color: rgb(255, 0, 0);">
                Test
            </div>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_html_vue_component() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.vue").unwrap();

        let html = r#"
            <template>
                <div style="--vue-var: blue;">Vue Component</div>
            </template>
            <style scoped>
                :root { --vue-style: red; }
            </style>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_html_svelte_component() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.svelte").unwrap();

        let html = r#"
            <div style="--svelte-var: green;">
                Svelte Component
            </div>
            <style>
                :global(:root) { --svelte-global: purple; }
            </style>
        "#;

        parse_html_document(html, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }
}
