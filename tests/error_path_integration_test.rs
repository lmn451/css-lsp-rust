use css_variable_lsp::manager::CssVariableManager;
use css_variable_lsp::parsers::{parse_css_document, parse_html_document};
use css_variable_lsp::types::Config;
use ls_types::Uri;
use std::str::FromStr;

#[tokio::test]
async fn test_css_parser_malformed_selectors() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let malformed_css = r#"
        :root {
            --color: red;
        }
        
        .button { {{ {
            background: var(--color);
        }
        
        .card .content > h1::before {
            --spacing: 1rem;
        }
    "#;

    let result = parse_css_document(malformed_css, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(all_vars.len() >= 2);

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--color".to_string()));
    assert!(var_names.contains(&"--spacing".to_string()));
}

#[tokio::test]
async fn test_css_parser_malformed_values() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let malformed_css = r#"
        :root {
            --color: red;
            --broken: {{ unexpected syntax }};
            --spacing: 1rem;
        }
        
        .test {
            background: var(--color);
            border: var(--broken, solid);
            padding: var(--spacing);
        }
    "#;

    let result = parse_css_document(malformed_css, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    let color_defs = manager.get_variables("--color").await;
    let spacing_defs = manager.get_variables("--spacing").await;
    let color_usages = manager.get_usages("--color").await;
    let spacing_usages = manager.get_usages("--spacing").await;

    assert!(all_vars.len() >= 2);
    assert_eq!(color_defs.len(), 1);
    assert_eq!(spacing_defs.len(), 1);
    assert_eq!(color_usages.len(), 1);
    assert_eq!(spacing_usages.len(), 1);
}

#[tokio::test]
async fn test_css_parser_unclosed_blocks() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let unclosed_css = r#"
        :root {
            --color: red;
        }
        
        .button {
            background: var(--color);
        }
        
        /* Unclosed comment
        
        .card {
            --spacing: 1rem;
            padding: var(--spacing);
        }
    "#;

    let result = parse_css_document(unclosed_css, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(!all_vars.is_empty());

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--color".to_string()));
}

#[tokio::test]
async fn test_css_parser_invalid_var_references() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let css_with_invalid_vars = r#"
        :root {
            --color: red;
        }
        
        .test {
            background: var(--color);
            border: var(--valid-name);
            padding: var();
            margin: var(--);
            outline: var(invalid syntax);
        }
    "#;

    let result = parse_css_document(css_with_invalid_vars, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--color".to_string()));
    assert!(!all_vars.is_empty());
}

#[tokio::test]
async fn test_html_parser_malformed_structure() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.html").unwrap();

    let malformed_html = r#"
        <html>
        <head>
            <style>
                :root {
                    --theme-color: blue;
                }
            </style>
        </head>
        <body>
            <div style="color: var(--theme-color);">
                Test content
            </div>
            <div style="--inline-var: red; color: var(--inline-var);">
                Another test
            </div>
            <!-- Unclosed comment
            <div style="broken syntax: {{ }}">
                Broken content
            </div>
        </body>
        "#;

    let result = parse_html_document(malformed_html, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(all_vars.len() >= 2);

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--theme-color".to_string()));
    assert!(var_names.contains(&"--inline-var".to_string()));
}

#[tokio::test]
async fn test_html_parser_malformed_style_tags() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.html").unwrap();

    let html_with_broken_styles = r#"
        <html>
        <head>
            <style type="text/css">
                :root {
                    --color: red;
                }
            </style>
            <style>
                --spacing: 1rem;
                background: var(--color);
            </style>
            <script>
                :root {
                    --script-var: green;
                }
            </script>
            <style>
                --another: blue;
                broken {{ }}
            </style>
        </head>
        <body>
            <div style="color: var(--color); padding: var(--spacing);">
                Content
            </div>
        </body>
        </html>
    "#;

    let result = parse_html_document(html_with_broken_styles, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(!all_vars.is_empty());

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--color".to_string()));
    assert!(!all_vars.is_empty());
}

#[tokio::test]
async fn test_css_parser_empty_and_whitespace() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let edge_case_css = r#"
        
        :root {
            --color: red;
        }
        
        .empty {
            
        }
        
        .whitespace {
            background:    var(--color)    ;
            padding:  var(--spacing)  ;
        }
        
    "#;

    let result = parse_css_document(edge_case_css, &uri, &manager).await;

    assert!(result.is_ok());

    let color_defs = manager.get_variables("--color").await;
    let color_usages = manager.get_usages("--color").await;

    assert_eq!(color_defs.len(), 1);
    assert_eq!(color_usages.len(), 1);
}

#[tokio::test]
async fn test_html_parser_missing_tags() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.html").unwrap();

    let incomplete_html = r#"
        <head>
            <style>
                :root {
                    --theme-color: blue;
                }
        <body>
            <div style="color: var(--theme-color);">
                Missing closing tags
            </div>
            <div style="--inline: red; color: var(--inline);">
                Another div
        "#;

    let result = parse_html_document(incomplete_html, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(!all_vars.is_empty());
}

#[tokio::test]
async fn test_css_parser_unicode_and_special_chars() {
    let manager = CssVariableManager::new(Config::default());
    let uri = Uri::from_str("file:///test.css").unwrap();

    let css_with_special_chars = r#"
        :root {
            --测试变量: red;
            --var-émojis: 🎨;
            --var-ñ: blue;
            --normal: green;
        }
        
        .test {
            background: var(--测试变量);
            color: var(--var-émojis);
            border: var(--var-ñ);
            outline: var(--normal);
        }
    "#;

    let result = parse_css_document(css_with_special_chars, &uri, &manager).await;

    assert!(result.is_ok());

    let all_vars = manager.get_all_variables().await;
    assert!(!all_vars.is_empty());

    let var_names: Vec<String> = all_vars.iter().map(|v| v.name.clone()).collect();
    assert!(var_names.contains(&"--normal".to_string()));
}
