use std::collections::HashMap;

use crate::document_kind::{resolve_document_kind, DocumentKind};
use crate::text_utils::{clamp_to_char_boundary, is_word_byte, is_word_char};
use crate::types::position_to_offset;
use ls_types::{Position, Uri};

pub struct CompletionContextSlice<'a> {
    pub slice: &'a str,
    pub allow_without_braces: bool,
}

pub struct ValueContext {
    pub is_value_context: bool,
    pub property_name: Option<String>,
}

pub fn completion_value_context_slice<'a>(
    text: &'a str,
    position: Position,
    language_id: Option<&str>,
    uri: &Uri,
    lookup_extension_map: &HashMap<String, DocumentKind>,
) -> Option<CompletionContextSlice<'a>> {
    let offset = position_to_offset(text, position)?;
    let offset = clamp_to_char_boundary(text, offset);
    let document_kind =
        resolve_document_kind(uri.path().as_str(), language_id, lookup_extension_map)?;
    let start = completion_lookback_start(text, offset, document_kind);
    let before_cursor = &text[start..offset];

    match document_kind {
        DocumentKind::Js => {
            let slice = find_js_string_segment(before_cursor)?;
            Some(CompletionContextSlice {
                slice,
                allow_without_braces: true,
            })
        }
        DocumentKind::Html => find_html_style_context_slice(before_cursor),
        DocumentKind::Css => Some(CompletionContextSlice {
            slice: before_cursor,
            allow_without_braces: false,
        }),
    }
}

fn completion_lookback_start(text: &str, offset: usize, document_kind: DocumentKind) -> usize {
    match document_kind {
        DocumentKind::Css => find_containing_block_start(text, offset),
        DocumentKind::Html => find_html_completion_lookback_start(text, offset),
        DocumentKind::Js => clamp_to_char_boundary(text, offset.saturating_sub(400)),
    }
}

/// Scan backward to the opening `{` of the block containing the cursor.
fn find_containing_block_start(text: &str, offset: usize) -> usize {
    let offset = clamp_to_char_boundary(text, offset.min(text.len()));
    let mut brace_depth = 0i32;

    for (idx, ch) in text[..offset].char_indices().rev() {
        match ch {
            '}' => brace_depth += 1,
            '{' => {
                if brace_depth == 0 {
                    return idx;
                }
                brace_depth -= 1;
            }
            _ => {}
        }
    }

    clamp_to_char_boundary(text, offset.saturating_sub(400))
}

/// For HTML, include the enclosing `<style>` tag when present so block parsing works.
fn find_html_completion_lookback_start(text: &str, offset: usize) -> usize {
    let offset = clamp_to_char_boundary(text, offset.min(text.len()));
    let lower = text[..offset].to_ascii_lowercase();

    if let Some(style_tag_idx) = lower.rfind("<style") {
        let inside_open_tag = lower
            .rfind("</style")
            .map(|close_idx| close_idx < style_tag_idx)
            .unwrap_or(true);
        if inside_open_tag {
            return style_tag_idx;
        }
    }

    find_containing_block_start(text, offset)
}

pub fn find_html_style_attribute_slice(before_cursor: &str) -> Option<&str> {
    let lower = before_cursor.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut search_end = lower.len();

    while let Some(idx) = lower[..search_end].rfind("style") {
        if idx > 0 && is_word_byte(bytes[idx - 1]) {
            search_end = idx;
            continue;
        }
        let after_idx = idx + 5;
        if after_idx < bytes.len() && is_word_byte(bytes[after_idx]) {
            search_end = idx;
            continue;
        }

        let mut j = after_idx;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            search_end = idx;
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }

        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            search_end = idx;
            continue;
        }
        let value_start = j + 1;
        let rest = &bytes[value_start..];
        if !rest.contains(&quote) {
            return Some(&before_cursor[value_start..]);
        }

        search_end = idx;
    }

    None
}

pub fn find_html_style_block_slice(before_cursor: &str) -> Option<&str> {
    let lower = before_cursor.to_ascii_lowercase();
    let open_idx = lower.rfind("<style")?;
    if let Some(close_idx) = lower.rfind("</style") {
        if close_idx > open_idx {
            return None;
        }
    }

    let tag_end_rel = lower[open_idx..].find('>')?;
    let tag_end = open_idx + tag_end_rel;
    if tag_end + 1 > before_cursor.len() {
        return None;
    }

    Some(&before_cursor[tag_end + 1..])
}

pub fn find_html_style_context_slice(before_cursor: &str) -> Option<CompletionContextSlice<'_>> {
    if let Some(slice) = find_html_style_attribute_slice(before_cursor) {
        return Some(CompletionContextSlice {
            slice,
            allow_without_braces: true,
        });
    }
    if let Some(slice) = find_html_style_block_slice(before_cursor) {
        return Some(CompletionContextSlice {
            slice,
            allow_without_braces: false,
        });
    }
    None
}

pub fn find_js_string_segment(before_cursor: &str) -> Option<&str> {
    let bytes = before_cursor.as_bytes();
    let mut in_quote: Option<u8> = None;
    let mut in_template = false;
    let mut template_expr_depth: i32 = 0;
    let mut expr_quote: Option<u8> = None;
    let mut segment_start: Option<usize> = None;

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_quote {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == q {
                in_quote = None;
                segment_start = None;
            }
            i += 1;
            continue;
        }

        if in_template {
            if template_expr_depth > 0 {
                if let Some(q) = expr_quote {
                    if b == b'\\' {
                        i = i.saturating_add(2);
                        continue;
                    }
                    if b == q {
                        expr_quote = None;
                    }
                    i += 1;
                    continue;
                }

                if b == b'\'' || b == b'"' || b == b'`' {
                    expr_quote = Some(b);
                    i += 1;
                    continue;
                }
                if b == b'{' {
                    template_expr_depth += 1;
                } else if b == b'}' {
                    template_expr_depth -= 1;
                    if template_expr_depth == 0 {
                        segment_start = Some(i + 1);
                    }
                }
                i += 1;
                continue;
            }

            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'`' {
                in_template = false;
                segment_start = None;
                i += 1;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                template_expr_depth = 1;
                segment_start = None;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if b == b'\'' || b == b'"' {
            in_quote = Some(b);
            segment_start = Some(i + 1);
            i += 1;
            continue;
        }
        if b == b'`' {
            in_template = true;
            segment_start = Some(i + 1);
            i += 1;
            continue;
        }
        i += 1;
    }

    if in_quote.is_some() {
        return segment_start.map(|start| &before_cursor[start..]);
    }
    if in_template && template_expr_depth == 0 {
        return segment_start.map(|start| &before_cursor[start..]);
    }
    None
}

pub fn find_context_colon(before_cursor: &str, allow_without_braces: bool) -> Option<usize> {
    let mut in_braces = 0i32;
    let mut in_parens = 0i32;
    let mut last_colon: i32 = -1;
    let mut last_semicolon: i32 = -1;
    let mut last_brace: i32 = -1;

    for (idx, ch) in before_cursor.char_indices().rev() {
        match ch {
            ')' => in_parens += 1,
            '(' => {
                in_parens -= 1;
                if in_parens < 0 {
                    in_parens = 0;
                }
            }
            '}' => in_braces += 1,
            '{' => {
                in_braces -= 1;
                if in_braces < 0 {
                    last_brace = idx as i32;
                    break;
                }
            }
            ':' if in_parens == 0 && in_braces == 0 && last_colon == -1 => {
                last_colon = idx as i32;
            }
            ';' if in_parens == 0 && in_braces == 0 && last_semicolon == -1 => {
                last_semicolon = idx as i32;
            }
            _ => {}
        }
    }

    if !allow_without_braces && last_brace == -1 {
        return None;
    }

    if last_colon > last_semicolon && last_colon > last_brace {
        Some(last_colon as usize)
    } else {
        None
    }
}

pub fn get_value_context_info(before_cursor: &str, allow_without_braces: bool) -> ValueContext {
    let colon_pos = match find_context_colon(before_cursor, allow_without_braces) {
        Some(pos) => pos,
        None => {
            return ValueContext {
                is_value_context: false,
                property_name: None,
            }
        }
    };
    let before_colon = before_cursor[..colon_pos].trim_end();
    if before_colon.is_empty() {
        return ValueContext {
            is_value_context: true,
            property_name: None,
        };
    }

    let mut start = before_colon.len();
    for (idx, ch) in before_colon.char_indices().rev() {
        if is_word_char(ch) {
            start = idx;
        } else {
            break;
        }
    }

    if start >= before_colon.len() {
        return ValueContext {
            is_value_context: true,
            property_name: None,
        };
    }

    ValueContext {
        is_value_context: true,
        property_name: Some(before_colon[start..].to_lowercase()),
    }
}

pub fn score_variable_relevance(var_name: &str, property_name: Option<&str>) -> i32 {
    let property_name = match property_name {
        Some(name) => name,
        None => return -1,
    };

    let lower_var_name = var_name.to_lowercase();

    let color_properties = [
        "color",
        "background-color",
        "background",
        "border-color",
        "outline-color",
        "text-decoration-color",
        "fill",
        "stroke",
    ];
    if color_properties.contains(&property_name) {
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
            || lower_var_name.contains("primary")
            || lower_var_name.contains("secondary")
            || lower_var_name.contains("accent")
            || lower_var_name.contains("text")
            || lower_var_name.contains("border")
            || lower_var_name.contains("link")
        {
            return 10;
        }
        if lower_var_name.contains("spacing")
            || lower_var_name.contains("margin")
            || lower_var_name.contains("padding")
            || lower_var_name.contains("size")
            || lower_var_name.contains("width")
            || lower_var_name.contains("height")
            || lower_var_name.contains("font")
            || lower_var_name.contains("weight")
            || lower_var_name.contains("radius")
        {
            return 0;
        }
        return 5;
    }

    let spacing_properties = [
        "margin",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "padding",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "gap",
        "row-gap",
        "column-gap",
    ];
    if spacing_properties.contains(&property_name) {
        if lower_var_name.contains("spacing")
            || lower_var_name.contains("margin")
            || lower_var_name.contains("padding")
            || lower_var_name.contains("gap")
        {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    let size_properties = [
        "width",
        "height",
        "max-width",
        "max-height",
        "min-width",
        "min-height",
        "font-size",
    ];
    if size_properties.contains(&property_name) {
        if lower_var_name.contains("width")
            || lower_var_name.contains("height")
            || lower_var_name.contains("size")
        {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    if property_name.contains("radius") {
        if lower_var_name.contains("radius") || lower_var_name.contains("rounded") {
            return 10;
        }
        if lower_var_name.contains("color")
            || lower_var_name.contains("bg")
            || lower_var_name.contains("background")
        {
            return 0;
        }
        return 5;
    }

    let font_properties = ["font-family", "font-weight", "font-style"];
    if font_properties.contains(&property_name) {
        if lower_var_name.contains("font") {
            return 10;
        }
        if lower_var_name.contains("color") || lower_var_name.contains("spacing") {
            return 0;
        }
        return 5;
    }

    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_kind::build_lookup_extension_map;
    use crate::types::{offset_to_position, Config};
    use ls_types::Uri;
    use std::str::FromStr;

    fn build_long_css_rule() -> String {
        let decl = "  margin-top: 1px;\n";
        let mut rule = String::from(".card {\n");
        for _ in 0..30 {
            rule.push_str(decl);
        }
        rule.push_str("  font: 400 16px/1.5 system-ui, sans-serif;\n");
        rule.push_str("  color: var(--");
        rule
    }

    #[test]
    fn long_css_rule_detects_value_context_at_bottom() {
        let text = build_long_css_rule();
        assert!(text.len() > 500);

        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let uri = Uri::from_str("file:///styles.css").unwrap();
        let position = offset_to_position(&text, text.len());

        let context = completion_value_context_slice(&text, position, None, &uri, &lookup_map)
            .expect("css document should yield a completion slice");
        let value_context = get_value_context_info(context.slice, context.allow_without_braces);

        assert!(
            value_context.is_value_context,
            "long rule blocks must still detect property value context"
        );
        assert_eq!(value_context.property_name.as_deref(), Some("color"));
    }

    #[test]
    fn nested_css_rule_detects_inner_property() {
        let text = ".outer { .inner { color: var(--";
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let uri = Uri::from_str("file:///styles.css").unwrap();
        let position = offset_to_position(text, text.len());

        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected css slice");
        let value_context = get_value_context_info(context.slice, context.allow_without_braces);

        assert!(value_context.is_value_context);
        assert_eq!(value_context.property_name.as_deref(), Some("color"));
    }

    #[test]
    fn html_style_block_lookback_includes_style_tag() {
        let text = "<style>body { color: var(";
        let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
        let uri = Uri::from_str("file:///index.html").unwrap();
        let position = offset_to_position(text, text.len());

        let context = completion_value_context_slice(text, position, None, &uri, &lookup_map)
            .expect("expected html style block slice");
        assert_eq!(context.slice, "body { color: var(");
        assert!(!context.allow_without_braces);
    }
}
