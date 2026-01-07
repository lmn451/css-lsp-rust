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

/// Integration test: CSS variables in @media queries
#[tokio::test]
async fn test_css_variables_in_media_queries() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///media.css").unwrap();

    let css_content = r#"
        :root {
            --breakpoint-md: 768px;
            --theme-color: blue;
        }

        @media (min-width: var(--breakpoint-md)) {
            .responsive {
                --responsive-padding: 2rem;
                color: var(--theme-color);
            }
        }

        @media screen and (max-width: 480px) {
            .mobile {
                --mobile-spacing: var(--responsive-padding, 1rem);
            }
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify variables defined in @media queries
    let responsive_defs = manager.get_variables("--responsive-padding").await;
    assert_eq!(responsive_defs.len(), 1);
    // Variables in @media queries should be properly detected
    assert!(!responsive_defs[0].selector.is_empty());

    // Verify usages in @media queries
    let theme_usages = manager.get_usages("--theme-color").await;
    assert_eq!(theme_usages.len(), 1);

    // Verify variables used within @media queries
    let responsive_usages = manager.get_usages("--responsive-padding").await;
    assert_eq!(responsive_usages.len(), 1);
}

/// Integration test: CSS variables in @keyframes
#[tokio::test]
async fn test_css_variables_in_keyframes() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///keyframes.css").unwrap();

    let css_content = r#"
        :root {
            --animation-duration: 0.3s;
            --start-color: red;
            --end-color: blue;
        }

        @keyframes slide-in {
            0% {
                --current-color: var(--start-color);
                transform: translateX(-100%);
            }
            100% {
                --current-color: var(--end-color);
                transform: translateX(0);
            }
        }

        .animated {
            animation: slide-in var(--animation-duration) ease-in-out;
            color: var(--current-color, black);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify variables in keyframes are tracked
    let current_color_defs = manager.get_variables("--current-color").await;
    assert_eq!(current_color_defs.len(), 2); // One for 0% and one for 100%

    // Verify usages in keyframes
    let start_color_usages = manager.get_usages("--start-color").await;
    assert_eq!(start_color_usages.len(), 1);

    let end_color_usages = manager.get_usages("--end-color").await;
    assert_eq!(end_color_usages.len(), 1);
}

/// Integration test: Complex CSS selectors
#[tokio::test]
async fn test_complex_css_selectors() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///complex.css").unwrap();

    let css_content = r#"
        :root {
            --primary: blue;
        }

        /* Complex selectors */
        .component[data-theme="dark"] .header > h1:first-child,
        .component[data-theme="light"] .header > h1:first-child {
            --header-color: var(--primary);
            color: var(--header-color);
        }

        .card:hover:not(.disabled)::before {
            --border-color: #ccc;
            border-color: var(--border-color);
        }

        /* Nested selectors with combinators */
        .parent .child + .sibling {
            --spacing: 1rem;
            margin: var(--spacing);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify variables are defined with complex selectors
    let header_color_defs = manager.get_variables("--header-color").await;
    assert_eq!(header_color_defs.len(), 1);
    // The selector should be extracted correctly from the complex selector list

    let border_color_defs = manager.get_variables("--border-color").await;
    assert_eq!(border_color_defs.len(), 1);

    // Verify usages
    let primary_usages = manager.get_usages("--primary").await;
    assert_eq!(primary_usages.len(), 1);
}

/// Integration test: CSS variables in calc() expressions
#[tokio::test]
async fn test_css_variables_in_calc_expressions() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///calc.css").unwrap();

    let css_content = r#"
        :root {
            --base-spacing: 1rem;
            --scale-factor: 1.5;
        }

        .element {
            --computed-width: calc(100% - var(--base-spacing) * 2);
            --scaled-size: calc(var(--base-spacing) * var(--scale-factor));
            width: var(--computed-width);
            font-size: var(--scaled-size);
        }

        .nested {
            --complex-calc: calc(var(--base-spacing) + 0.5rem);
            padding: var(--complex-calc);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify variables with calc() expressions are parsed
    let computed_defs = manager.get_variables("--computed-width").await;
    assert_eq!(computed_defs.len(), 1);
    assert!(computed_defs[0].value.contains("calc("));

    // Verify usages in calc expressions
    let base_spacing_usages = manager.get_usages("--base-spacing").await;
    assert_eq!(base_spacing_usages.len(), 3); // Used in computed-width, scaled-size, and complex-calc

    let scale_usages = manager.get_usages("--scale-factor").await;
    assert_eq!(scale_usages.len(), 1);
}

/// Integration test: CSS variables in grid and flexbox properties
#[tokio::test]
async fn test_css_variables_in_layout_properties() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///layout.css").unwrap();

    let css_content = r#"
        :root {
            --grid-columns: 1fr 2fr 1fr;
            --flex-gap: 1rem;
            --min-width: 200px;
        }

        .grid-container {
            --grid-template: var(--grid-columns) / 1fr;
            grid-template: var(--grid-template);
            gap: var(--flex-gap);
        }

        .flex-container {
            --flex-layout: row nowrap;
            flex-flow: var(--flex-layout);
            gap: var(--flex-gap);
        }

        .constrained {
            --size-constraint: minmax(var(--min-width), 1fr);
            grid-template-columns: var(--size-constraint);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify complex layout variables
    let grid_template_defs = manager.get_variables("--grid-template").await;
    assert_eq!(grid_template_defs.len(), 1);

    let flex_layout_defs = manager.get_variables("--flex-layout").await;
    assert_eq!(flex_layout_defs.len(), 1);

    // Verify usages in layout properties
    let grid_columns_usages = manager.get_usages("--grid-columns").await;
    assert_eq!(grid_columns_usages.len(), 1);

    let flex_gap_usages = manager.get_usages("--flex-gap").await;
    assert_eq!(flex_gap_usages.len(), 2); // Used in both grid and flex containers

    let min_width_usages = manager.get_usages("--min-width").await;
    assert_eq!(min_width_usages.len(), 1);
}

/// Integration test: CSS custom properties with special characters and escaping
#[tokio::test]
async fn test_css_custom_properties_edge_cases() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Url::parse("file:///edge-cases.css").unwrap();

    let css_content = r#"
        :root {
            --simple: blue;
            --with-dashes: red;
            --with_underscores: green;
            --with-numbers123: yellow;
            --mixed-123_abc: purple;
        }

        .test {
            color: var(--simple);
            background: var(--with-dashes);
            border-color: var(--with_underscores);
            outline-color: var(--with-numbers123);
            box-shadow: 0 0 10px var(--mixed-123_abc);
        }

        /* Variables that might be confused with other syntax */
        .special {
            --calc-like: calc(100% - 20px);
            --url-like: url(https://example.com);
            --function-like: rgba(255, 0, 0, 0.5);
            background: var(--calc-like) var(--url-like) var(--function-like);
        }
    "#;

    parse_css_document(css_content, &uri, &manager)
        .await
        .unwrap();

    // Verify all variable names are parsed correctly
    let all_vars = manager.get_all_variables().await;
    assert_eq!(all_vars.len(), 8); // 5 definitions + 3 special ones

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--simple".to_string()));
    assert!(var_names.contains(&"--with-dashes".to_string()));
    assert!(var_names.contains(&"--with_underscores".to_string()));
    assert!(var_names.contains(&"--with-numbers123".to_string()));
    assert!(var_names.contains(&"--mixed-123_abc".to_string()));

    // Verify usages are tracked correctly
    let simple_usages = manager.get_usages("--simple").await;
    assert_eq!(simple_usages.len(), 1);

    let mixed_usages = manager.get_usages("--mixed-123_abc").await;
    assert_eq!(mixed_usages.len(), 1);
}
