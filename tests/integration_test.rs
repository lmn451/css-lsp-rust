use css_variable_lsp::manager::CssVariableManager;
use css_variable_lsp::parsers::{parse_css_document, parse_html_document};
use css_variable_lsp::types::Config;
use tower_lsp::lsp_types::Url;

/// Integration test: Full CSS variable workflow
#[tokio::test]
async fn test_css_variable_full_workflow() {
    let manager = CssVariableManager::new(Config::default());
    let css_uri = Url::parse("file:///test.css").unwrap();
    let html_uri = Url::parse("file:///test.html").unwrap();

    // Parse CSS file with variable definitions
    let css_content = r#"
        :root {
            --primary-color: #3b82f6;
            --secondary-color: #8b5cf6;
            --spacing: 1rem;
        }
        
        .button {
            background: var(--primary-color);
            padding: var(--spacing);
        }
        
        .card {
            color: var(--undefined-variable);
            border-color: var(--secondary-color, #ccc);
        }
    "#;

    parse_css_document(css_content, &css_uri, &manager)
        .await
        .unwrap();

    // Parse HTML file with inline styles
    let html_content = r#"
        <html>
            <head>
                <style>
                    .header {
                        --header-bg: #fff;
                        background: var(--header-bg);
                    }
                </style>
            </head>
            <body>
                <div style="--inline-color: red; color: var(--inline-color);">
                    Test
                </div>
            </body>
        </html>
    "#;

    parse_html_document(html_content, &html_uri, &manager)
        .await
        .unwrap();

    // Test 1: Verify definitions are extracted
    let primary_defs = manager.get_variables("--primary-color").await;
    assert_eq!(primary_defs.len(), 1);
    assert_eq!(primary_defs[0].value.trim(), "#3b82f6");
    assert_eq!(primary_defs[0].selector, ":root");

    let header_defs = manager.get_variables("--header-bg").await;
    assert_eq!(header_defs.len(), 1);
    assert_eq!(header_defs[0].value.trim(), "#fff");

    let inline_defs = manager.get_variables("--inline-color").await;
    assert_eq!(inline_defs.len(), 1);
    assert_eq!(inline_defs[0].value.trim(), "red");
    assert!(inline_defs[0].inline);

    // Test 2: Verify usages are tracked
    let primary_usages = manager.get_usages("--primary-color").await;
    assert_eq!(primary_usages.len(), 1);
    assert_eq!(primary_usages[0].usage_context, ".button");

    let undefined_usages = manager.get_usages("--undefined-variable").await;
    assert_eq!(undefined_usages.len(), 1);

    // Test 3: Verify references (definitions + usages)
    let (defs, usages) = manager.get_references("--secondary-color").await;
    assert_eq!(defs.len(), 1);
    assert_eq!(usages.len(), 1);

    // Test 4: Verify document removal
    manager.remove_document(&css_uri).await;
    let primary_after_removal = manager.get_variables("--primary-color").await;
    assert_eq!(primary_after_removal.len(), 0);

    // HTML variables should still exist
    let header_after_removal = manager.get_variables("--header-bg").await;
    assert_eq!(header_after_removal.len(), 1);
}

/// Integration test: CSS specificity and cascade ordering
#[tokio::test]
async fn test_cascade_ordering() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///cascade.css").unwrap();

    let css_content = r#"
        :root {
            --color: blue;
        }
        
        .class {
            --color: green;
        }
        
        #id {
            --color: red !important;
        }
        
        div {
            --color: yellow;
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    let color_defs = manager.get_variables("--color").await;
    assert_eq!(color_defs.len(), 4);

    // Verify all definitions exist
    let selectors: Vec<String> = color_defs.iter().map(|d| d.selector.clone()).collect();
    assert!(selectors.contains(&":root".to_string()));
    assert!(selectors.contains(&".class".to_string()));
    assert!(selectors.contains(&"#id".to_string()));
    assert!(selectors.contains(&"div".to_string()));

    // Verify !important flag is captured
    let important_count = color_defs.iter().filter(|d| d.important).count();
    assert_eq!(important_count, 1);
}

/// Integration test: Color resolution with var() chains
#[tokio::test]
async fn test_color_resolution_chain() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///colors.css").unwrap();

    let css_content = r#"
        :root {
            --base-color: #3b82f6;
            --primary: var(--base-color);
            --theme-color: var(--primary);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Test that we can resolve the chain
    let base = manager.get_variables("--base-color").await;
    assert_eq!(base.len(), 1);
    assert_eq!(base[0].value.trim(), "#3b82f6");

    let primary = manager.get_variables("--primary").await;
    assert_eq!(primary.len(), 1);
    assert!(primary[0].value.contains("var(--base-color)"));

    // Test color resolution
    let color = manager.resolve_variable_color("--theme-color").await;
    assert!(color.is_some());
}

/// Integration test: Multiple file types
#[tokio::test]
async fn test_multiple_file_types() {
    let manager = CssVariableManager::new(Config::default());
    
    // CSS file
    let css_uri = Url::parse("file:///styles.css").unwrap();
    let css_content = ":root { --css-var: blue; }";
    parse_css_document(css_content, &css_uri, &manager)
        .await
        .unwrap();
    
    // HTML file
    let html_uri = Url::parse("file:///index.html").unwrap();
    let html_content = r#"
        <style>:root { --html-var: red; }</style>
        <div style="--inline-var: green;"></div>
    "#;
    parse_html_document(html_content, &html_uri, &manager)
        .await
        .unwrap();

    // Verify all variables from different sources
    let all_vars = manager.get_all_variables().await;
    assert!(all_vars.len() >= 3);
    
    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--css-var".to_string()));
    assert!(var_names.contains(&"--html-var".to_string()));
    assert!(var_names.contains(&"--inline-var".to_string()));
}

/// Integration test: Fallback handling in var()
#[tokio::test]
async fn test_var_fallback_handling() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///fallback.css").unwrap();

    let css_content = r#"
        .button {
            color: var(--primary-color, blue);
            background: var(--bg-color, var(--fallback, #fff));
            padding: var(--spacing);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify usages are tracked (but not nested fallbacks)
    let primary_usages = manager.get_usages("--primary-color").await;
    assert_eq!(primary_usages.len(), 1);
    
    let bg_usages = manager.get_usages("--bg-color").await;
    assert_eq!(bg_usages.len(), 1);
    
    let spacing_usages = manager.get_usages("--spacing").await;
    assert_eq!(spacing_usages.len(), 1);
    
    // Nested fallback should not be tracked as separate usage
    let fallback_usages = manager.get_usages("--fallback").await;
    assert_eq!(fallback_usages.len(), 0);
}
