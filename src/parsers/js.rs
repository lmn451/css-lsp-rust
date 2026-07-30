use ls_types::Uri;

use super::css::{parse_css_snippet, CssParseContext};
use crate::manager::CssVariableManager;

/// A CSS snippet extracted from a JS/TS source file (e.g. styled-components).
pub(crate) struct JsCssSnippet {
    /// Byte offset where the CSS content starts in the full document.
    pub content_start: usize,
    /// The CSS content with template expressions blanked out (spaces).
    pub content: String,
}

/// Parse a JS/TS document and extract CSS from tagged template literals and string literals.
pub async fn parse_js_document(
    text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
) -> Result<(), String> {
    let snippets = extract_js_css_snippets(text);
    let mut parse_errors = 0;
    for snippet in snippets {
        let context = CssParseContext {
            css_text: &snippet.content,
            full_text: text,
            uri,
            manager,
            base_offset: snippet.content_start,
            inline: false,
            usage_context_override: Some("js-template"),
            dom_node: None,
        };
        if let Err(e) = parse_css_snippet(context).await {
            tracing::debug!("JS parse error at offset {}: {}", snippet.content_start, e);
            parse_errors += 1;
        }
    }
    if parse_errors > 0 {
        tracing::warn!(
            "Encountered {} parse errors in JS document {:?}",
            parse_errors,
            uri
        );
    }
    Ok(())
}

/// Heuristic: does this string contain CSS-like content?
/// Avoids false positives like "user:pass", "https://", etc.
fn has_css_like_content(s: &str) -> bool {
    // Must have colon with proper context (not URL protocol) OR contain CSS patterns
    // CSS properties have colons with property names (letter sequence before colon)
    // vs URLs have protocol prefix (://)

    // Contains var() or --custom-property syntax (definite CSS)
    if s.contains("var(") || s.contains("--") {
        return true;
    }

    // Contains colon - need to check it's not a protocol or credential
    if let Some(pos) = s.find(':') {
        // Check what follows the colon
        let after = &s[pos + 1..].trim_start();
        // URL protocol pattern: "://" or just "//" at start
        if s.starts_with("http") || s.starts_with("//") {
            return false;
        }
        // Check it's not a credential pattern (word:word without space after colon)
        // CSS property: "prop: value" has space after colon
        // Credential: "user:pass" no space
        if !after.starts_with(' ') && !after.starts_with(';') && !after.is_empty() {
            // No space after colon - could be credential, check for common URL patterns
            if s.contains("://") || s.starts_with('/') {
                return false;
            }
        }
    }

    // Fallback to original logic for backward compatibility
    s.contains(':') || s.contains("--") || s.contains("var(")
}

fn append_blank_bytes(content: &mut String, byte_count: usize) {
    content.extend(std::iter::repeat_n(' ', byte_count));
}

/// Extract all CSS-like string/template literal snippets from a JS source.
pub(crate) fn extract_js_css_snippets(text: &str) -> Vec<JsCssSnippet> {
    let bytes = text.as_bytes();
    let mut snippets = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        match b {
            b'\'' | b'"' => {
                // Regular string literal
                let quote = b;
                let content_start = i + 1;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        let content = &text[content_start..i];
                        if has_css_like_content(content) {
                            snippets.push(JsCssSnippet {
                                content_start,
                                content: content.to_string(),
                            });
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'`' => {
                // Template literal — handle ${…} expressions
                let content_start = i + 1;
                let mut content = String::with_capacity(64);
                let mut expr_depth: i32 = 0;
                // Track nested quote within expressions
                let mut expr_quote: Option<u8> = None;
                i += 1;

                while i < bytes.len() {
                    if expr_depth > 0 {
                        // Blank every consumed source byte so later offsets remain stable.
                        if let Some(q) = expr_quote {
                            if bytes[i] == b'\\' {
                                let consumed = (bytes.len() - i).min(2);
                                append_blank_bytes(&mut content, consumed);
                                i += consumed;
                                continue;
                            }
                            append_blank_bytes(&mut content, 1);
                            if bytes[i] == q {
                                expr_quote = None;
                            }
                            i += 1;
                            continue;
                        }
                        match bytes[i] {
                            b'\'' | b'"' | b'`' => {
                                expr_quote = Some(bytes[i]);
                                append_blank_bytes(&mut content, 1);
                                i += 1;
                                continue;
                            }
                            b'{' => {
                                expr_depth += 1;
                                append_blank_bytes(&mut content, 1);
                                i += 1;
                                continue;
                            }
                            b'}' => {
                                expr_depth -= 1;
                                append_blank_bytes(&mut content, 1);
                                i += 1;
                                continue;
                            }
                            _ => {
                                append_blank_bytes(&mut content, 1);
                                i += 1;
                                continue;
                            }
                        }
                    }

                    // Inside template literal (not in expression)
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'`' {
                        // End of template literal
                        if has_css_like_content(&content) {
                            snippets.push(JsCssSnippet {
                                content_start,
                                content,
                            });
                        }
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                        expr_depth = 1;
                        // Replace ${ with spaces to preserve offsets
                        content.push(' ');
                        content.push(' ');
                        i += 2;
                        continue;
                    }
                    // Safely handle multi-byte UTF-8 characters
                    if let Some(c) = text[i..].chars().next() {
                        content.push(c);
                        i += c.len_utf8();
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' => {
                // Skip comments to avoid false positives from URLs or regex
                if i + 1 < bytes.len() {
                    if bytes[i + 1] == b'/' {
                        i += 2;
                        while i < bytes.len() && bytes[i] != b'\n' {
                            i += 1;
                        }
                        continue;
                    }
                    if bytes[i + 1] == b'*' {
                        i += 2;
                        while i + 1 < bytes.len() {
                            if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                        continue;
                    }
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    snippets
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::manager::CssVariableManager;
    use crate::types::{offset_to_position, Config};
    use std::str::FromStr;

    #[test]
    fn test_extract_simple_template_literal() {
        let text = "const css = `color: red;`";
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].content, "color: red;");
    }

    #[test]
    fn test_extract_template_with_expressions() {
        let text = "const Btn = styled.button`\n  color: ${props => props.$color};\n  background: #3b82f6;\n`";
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 1);
        let c = &snippets[0].content;
        // The expression ${...} should be replaced with spaces
        assert!(c.contains("background: #3b82f6"));
        assert!(c.contains("color:"));
        // Expression region should be blanked
        assert!(!c.contains("props"));
    }

    async fn assert_variable_name_position(text: &str, name: &str) {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.ts").unwrap();
        parse_js_document(text, &uri, &manager).await.unwrap();
        let variables = manager.get_variables(name).await;
        assert_eq!(variables.len(), 1);
        let expected_offset = text.find(name).unwrap();
        assert_eq!(variables[0].source_position, expected_offset);
        assert_eq!(
            variables[0].name_range.unwrap().start,
            offset_to_position(text, expected_offset),
        );
    }

    #[tokio::test]
    async fn test_template_expression_preserves_following_range() {
        let text = r#"const css = `color: ${"red"}; --after: blue;`;"#;
        assert_variable_name_position(text, "--after").await;
    }

    #[tokio::test]
    async fn test_escaped_template_expression_preserves_following_range() {
        let text = r#"const css = `color: ${"re\"d"}; --after: blue;`;"#;
        assert_variable_name_position(text, "--after").await;
    }

    #[tokio::test]
    async fn test_multibyte_template_expression_preserves_following_range() {
        let text = r#"const css = `color: ${"赤色"}; --after: blue;`;"#;
        assert_variable_name_position(text, "--after").await;
    }

    #[test]
    fn test_extract_multiple_templates() {
        let text = r#"
            const a = styled.div`color: red;`;
            const b = styled.span`background: blue;`;
        "#;
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 2);
    }

    #[test]
    fn test_extract_string_literal() {
        let text = r#"const css = "color: #fff;""#;
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].content, "color: #fff;");
    }

    #[test]
    fn test_skip_non_css_strings() {
        let text = r#"const msg = "hello world";"#;
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 0);
    }

    #[test]
    fn test_skip_comments() {
        let text = "// this is a `comment` with a backtick\nconst css = `color: red;`";
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].content, "color: red;");
    }

    #[test]
    fn test_template_nested_braces_in_expression() {
        let text = "const css = `color: ${({theme}) => theme.primary};`";
        let snippets = extract_js_css_snippets(text);
        assert_eq!(snippets.len(), 1);
        // The content should still be recognized as CSS (has colon)
        assert!(snippets[0].content.contains("color:"));
    }
}
