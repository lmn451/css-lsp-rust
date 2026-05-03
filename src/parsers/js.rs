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
        let _ = parse_css_snippet(context).await;
    }
    Ok(())
}

/// Heuristic: does this string contain CSS-like content?
fn has_css_like_content(s: &str) -> bool {
    // Look for colon (property separator) or `--` (custom property) or `var(`
    s.contains(':') || s.contains("--") || s.contains("var(")
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
                        // Inside a template expression
                        if let Some(q) = expr_quote {
                            // Inside a nested string within the expression
                            if bytes[i] == b'\\' {
                                i += 2;
                                continue;
                            }
                            if bytes[i] == q {
                                expr_quote = None;
                            }
                            i += 1;
                            continue;
                        }

                        match bytes[i] {
                            b'\'' | b'"' | b'`' => {
                                expr_quote = Some(bytes[i]);
                                i += 1;
                                continue;
                            }
                            b'{' => {
                                expr_depth += 1;
                                i += 1;
                                continue;
                            }
                            b'}' => {
                                expr_depth -= 1;
                                if expr_depth == 0 {
                                    // Expression closed; push a space to preserve
                                    // offsets for the literal content that follows
                                    content.push(' ');
                                    i += 1;
                                    continue;
                                }
                                i += 1;
                                continue;
                            }
                            _ => {
                                // Replace expression characters with spaces
                                // to keep position tracking accurate
                                content.push(' ');
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
                    content.push(bytes[i] as char);
                    i += 1;
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
