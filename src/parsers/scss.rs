use tower_lsp::lsp_types::{Range, Url};

use crate::manager::CssVariableManager;
use crate::types::{offset_to_position, CssVariable, CssVariableUsage, VariableKind};

/// Parse an SCSS/SASS document and extract variable definitions and usages.
///
/// This parser extracts:
/// - SCSS variable definitions: `$variable-name: value;`
/// - SCSS variable usages: `$variable-name` in property values
/// - CSS custom properties (delegates to CSS parser for `--var` syntax)
///
/// Note: This is a simplified parser that treats all SCSS variables as global scope.
/// It handles `!default` and `!global` flags but doesn't track block-level scoping.
pub async fn parse_scss_document(
    text: &str,
    uri: &Url,
    manager: &CssVariableManager,
) -> Result<(), String> {
    // First, parse CSS custom properties (--var) using the CSS parser
    // This ensures we still support CSS variables in SCSS files
    crate::parsers::css::parse_css_document(text, uri, manager).await?;

    // Then parse SCSS-specific $variable syntax
    extract_scss_definitions(text, uri, manager).await;
    extract_scss_usages(text, uri, manager).await;

    Ok(())
}

/// Extract SCSS variable definitions ($variable: value;)
async fn extract_scss_definitions(text: &str, uri: &Url, manager: &CssVariableManager) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_block_comment = false;
    let mut in_line_comment = false;
    let mut in_string: Option<u8> = None;

    while i < len {
        // Handle line comments (//)
        if in_line_comment {
            if bytes[i] == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        // Handle block comments (/* */)
        if in_block_comment {
            if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Handle strings
        if let Some(quote) = in_string {
            if bytes[i] == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        // Start of line comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }

        // Start of block comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }

        // Start of string
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        // Look for $ at the start of an identifier (potential definition)
        if bytes[i] == b'$' {
            let name_start = i;
            let mut j = i + 1;

            // Parse variable name
            while j < len && is_scss_ident_char(bytes[j]) {
                j += 1;
            }

            // Must have at least one character after $
            if j == name_start + 1 {
                i += 1;
                continue;
            }

            let name_end = j;

            // Skip whitespace
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }

            // Check for colon (definition)
            if j >= len || bytes[j] != b':' {
                // Not a definition, skip to after the name
                i = name_end;
                continue;
            }

            // This is a definition - parse the value
            let mut value_start = j + 1;
            while value_start < len && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }

            // Find the end of the value (semicolon, closing brace, or end of file)
            let mut value_end = value_start;
            let mut depth = 0i32;
            let mut val_in_string: Option<u8> = None;
            let mut val_in_line_comment = false;
            let mut val_in_block_comment = false;

            while value_end < len {
                let b = bytes[value_end];

                // Handle comments in value
                if val_in_line_comment {
                    if b == b'\n' {
                        val_in_line_comment = false;
                    }
                    value_end += 1;
                    continue;
                }

                if val_in_block_comment {
                    if value_end + 1 < len && b == b'*' && bytes[value_end + 1] == b'/' {
                        val_in_block_comment = false;
                        value_end += 2;
                        continue;
                    }
                    value_end += 1;
                    continue;
                }

                // Handle strings in value
                if let Some(q) = val_in_string {
                    if b == b'\\' && value_end + 1 < len {
                        value_end += 2;
                        continue;
                    }
                    if b == q {
                        val_in_string = None;
                    }
                    value_end += 1;
                    continue;
                }

                // Check for comment start
                if value_end + 1 < len && b == b'/' && bytes[value_end + 1] == b'/' {
                    val_in_line_comment = true;
                    value_end += 2;
                    continue;
                }
                if value_end + 1 < len && b == b'/' && bytes[value_end + 1] == b'*' {
                    val_in_block_comment = true;
                    value_end += 2;
                    continue;
                }

                // Check for string start
                if b == b'"' || b == b'\'' {
                    val_in_string = Some(b);
                    value_end += 1;
                    continue;
                }

                // Track parentheses depth
                if b == b'(' {
                    depth += 1;
                } else if b == b')' && depth > 0 {
                    depth -= 1;
                }

                // End of value
                if depth == 0 && (b == b';' || b == b'}' || b == b'\n') {
                    // For newline, only end if we're at top level and have content
                    if b == b'\n' && value_end > value_start {
                        // Check if line continues (e.g., with operators)
                        let trimmed = text[value_start..value_end].trim();
                        if !trimmed.ends_with(',')
                            && !trimmed.ends_with('+')
                            && !trimmed.ends_with('-')
                            && !trimmed.ends_with('*')
                            && !trimmed.ends_with('/')
                            && !trimmed.ends_with(':')
                        {
                            break;
                        }
                    } else if b != b'\n' {
                        break;
                    }
                }

                value_end += 1;
            }

            // Trim trailing whitespace from value
            let mut value_end_trim = value_end;
            while value_end_trim > value_start && bytes[value_end_trim - 1].is_ascii_whitespace() {
                value_end_trim -= 1;
            }

            let name = text[name_start..name_end].to_string();
            let value_str = text[value_start..value_end_trim].trim();

            // Check for !default and !global flags
            let (value, is_default, is_global) = parse_scss_flags(value_str);

            let variable = CssVariable {
                name,
                value: value.to_string(),
                uri: uri.clone(),
                range: Range::new(
                    offset_to_position(text, name_start),
                    offset_to_position(text, value_end_trim),
                ),
                name_range: Some(Range::new(
                    offset_to_position(text, name_start),
                    offset_to_position(text, name_end),
                )),
                value_range: Some(Range::new(
                    offset_to_position(text, value_start),
                    offset_to_position(text, value_end_trim),
                )),
                selector: String::new(),
                important: false,
                inline: false,
                source_position: name_start,
                kind: VariableKind::Scss,
                is_default,
                is_global,
                scope: None,
            };

            manager.add_variable(variable).await;

            // Move past the semicolon if present
            i = if value_end < len && bytes[value_end] == b';' {
                value_end + 1
            } else {
                value_end
            };
            continue;
        }

        i += 1;
    }
}

/// Extract SCSS variable usages ($variable in property values)
async fn extract_scss_usages(text: &str, uri: &Url, manager: &CssVariableManager) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_block_comment = false;
    let mut in_line_comment = false;
    let mut in_string: Option<u8> = None;
    let mut current_selector = String::new();
    let mut brace_depth: u32 = 0;

    while i < len {
        // Handle line comments
        if in_line_comment {
            if bytes[i] == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        // Handle block comments
        if in_block_comment {
            if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Handle strings (skip $ in strings)
        if let Some(quote) = in_string {
            if bytes[i] == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        // Start of line comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }

        // Start of block comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }

        // Start of string
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        // Track braces for selector context
        if bytes[i] == b'{' {
            // Capture selector before the brace
            if brace_depth == 0 {
                let start = text[..i].rfind(['}', ';', '{']);
                let selector_start = start.map(|s| s + 1).unwrap_or(0);
                current_selector = text[selector_start..i].trim().to_string();
            }
            brace_depth += 1;
            i += 1;
            continue;
        }

        if bytes[i] == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
            if brace_depth == 0 {
                current_selector.clear();
            }
            i += 1;
            continue;
        }

        // Look for $ (potential usage)
        if bytes[i] == b'$' {
            let var_start = i;
            let mut j = i + 1;

            // Parse variable name
            while j < len && is_scss_ident_char(bytes[j]) {
                j += 1;
            }

            // Must have at least one character after $
            if j == var_start + 1 {
                i += 1;
                continue;
            }

            let var_end = j;

            // Skip whitespace to check if this is a definition (has colon)
            let mut k = j;
            while k < len && bytes[k].is_ascii_whitespace() {
                k += 1;
            }

            // If followed by colon, this is a definition, not a usage
            if k < len && bytes[k] == b':' {
                i = var_end;
                continue;
            }

            // This is a usage
            let name = text[var_start..var_end].to_string();

            let usage = CssVariableUsage {
                name,
                uri: uri.clone(),
                range: Range::new(
                    offset_to_position(text, var_start),
                    offset_to_position(text, var_end),
                ),
                name_range: Some(Range::new(
                    offset_to_position(text, var_start),
                    offset_to_position(text, var_end),
                )),
                usage_context: current_selector.clone(),
                dom_node: None,
                kind: VariableKind::Scss,
            };

            manager.add_usage(usage).await;
            i = var_end;
            continue;
        }

        i += 1;
    }
}

/// Parse !default and !global flags from SCSS value
fn parse_scss_flags(value: &str) -> (&str, bool, bool) {
    let mut is_default = false;
    let mut is_global = false;
    let mut trimmed = value.trim();

    // Check for flags at the end of the value
    loop {
        if trimmed.ends_with("!default") {
            is_default = true;
            trimmed = trimmed[..trimmed.len() - 8].trim();
        } else if trimmed.ends_with("!global") {
            is_global = true;
            trimmed = trimmed[..trimmed.len() - 7].trim();
        } else {
            break;
        }
    }

    (trimmed, is_default, is_global)
}

/// Check if a byte is a valid SCSS identifier character
fn is_scss_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::CssVariableManager;
    use crate::types::Config;

    #[tokio::test]
    async fn test_parse_scss_variable_definitions() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            $primary-color: #3b82f6;
            $secondary-color: #10b981;
            $spacing: 1rem;
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let primary = manager.get_variables("$primary-color").await;
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].value, "#3b82f6");
        assert_eq!(primary[0].kind, VariableKind::Scss);

        let secondary = manager.get_variables("$secondary-color").await;
        assert_eq!(secondary.len(), 1);
        assert_eq!(secondary[0].value, "#10b981");

        let spacing = manager.get_variables("$spacing").await;
        assert_eq!(spacing.len(), 1);
        assert_eq!(spacing[0].value, "1rem");
    }

    #[tokio::test]
    async fn test_parse_scss_variable_with_flags() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            $default-color: blue !default;
            $global-var: red !global;
            $both-flags: green !default !global;
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let default_var = manager.get_variables("$default-color").await;
        assert_eq!(default_var.len(), 1);
        assert_eq!(default_var[0].value, "blue");
        assert!(default_var[0].is_default);
        assert!(!default_var[0].is_global);

        let global_var = manager.get_variables("$global-var").await;
        assert_eq!(global_var.len(), 1);
        assert_eq!(global_var[0].value, "red");
        assert!(!global_var[0].is_default);
        assert!(global_var[0].is_global);

        let both = manager.get_variables("$both-flags").await;
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].value, "green");
        assert!(both[0].is_default);
        assert!(both[0].is_global);
    }

    #[tokio::test]
    async fn test_parse_scss_variable_usages() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            $primary: blue;
            
            .button {
                color: $primary;
                background: lighten($primary, 10%);
            }
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let usages = manager.get_usages("$primary").await;
        assert_eq!(usages.len(), 2);
        assert!(usages.iter().all(|u| u.kind == VariableKind::Scss));
    }

    #[tokio::test]
    async fn test_parse_scss_with_comments() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            // Single line comment
            $color: red; // inline comment
            
            /* Block comment
               $ignored: value;
            */
            
            $another: blue;
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let color = manager.get_variables("$color").await;
        assert_eq!(color.len(), 1);

        let ignored = manager.get_variables("$ignored").await;
        assert_eq!(ignored.len(), 0);

        let another = manager.get_variables("$another").await;
        assert_eq!(another.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_scss_mixed_with_css_vars() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            $scss-var: blue;
            
            :root {
                --css-var: red;
            }
            
            .element {
                color: $scss-var;
                background: var(--css-var);
            }
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let scss_var = manager.get_variables("$scss-var").await;
        assert_eq!(scss_var.len(), 1);
        assert_eq!(scss_var[0].kind, VariableKind::Scss);

        let css_var = manager.get_variables("--css-var").await;
        assert_eq!(css_var.len(), 1);
        assert_eq!(css_var[0].kind, VariableKind::Css);
    }

    #[tokio::test]
    async fn test_parse_scss_complex_values() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Url::parse("file:///test.scss").unwrap();
        let text = r#"
            $gradient: linear-gradient(to right, red, blue);
            $shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
            $calc-value: calc(100% - 20px);
        "#;

        parse_scss_document(text, &uri, &manager).await.unwrap();

        let gradient = manager.get_variables("$gradient").await;
        assert_eq!(gradient.len(), 1);
        assert!(gradient[0].value.contains("linear-gradient"));

        let shadow = manager.get_variables("$shadow").await;
        assert_eq!(shadow.len(), 1);
        assert!(shadow[0].value.contains("rgba"));

        let calc = manager.get_variables("$calc-value").await;
        assert_eq!(calc.len(), 1);
        assert!(calc[0].value.contains("calc"));
    }

    #[test]
    fn test_parse_scss_flags() {
        assert_eq!(parse_scss_flags("blue"), ("blue", false, false));
        assert_eq!(parse_scss_flags("blue !default"), ("blue", true, false));
        assert_eq!(parse_scss_flags("blue !global"), ("blue", false, true));
        assert_eq!(
            parse_scss_flags("blue !default !global"),
            ("blue", true, true)
        );
        assert_eq!(
            parse_scss_flags("blue !global !default"),
            ("blue", true, true)
        );
    }

    #[test]
    fn test_is_scss_ident_char() {
        assert!(is_scss_ident_char(b'a'));
        assert!(is_scss_ident_char(b'Z'));
        assert!(is_scss_ident_char(b'0'));
        assert!(is_scss_ident_char(b'-'));
        assert!(is_scss_ident_char(b'_'));
        assert!(!is_scss_ident_char(b'$'));
        assert!(!is_scss_ident_char(b':'));
        assert!(!is_scss_ident_char(b' '));
    }
}
