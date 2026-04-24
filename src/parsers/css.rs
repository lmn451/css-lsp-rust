use ls_types::{Range, Uri};
use tracing::warn;

use crate::color::normalized_color_key;
use crate::manager::CssVariableManager;
use crate::types::{
    offset_to_position, CssVariable, CssVariableUsage, DOMNodeInfo, LiteralColorOccurrence,
};

/// Configuration for parsing CSS snippets
pub struct CssParseContext<'a> {
    pub css_text: &'a str,
    pub full_text: &'a str,
    pub uri: &'a Uri,
    pub manager: &'a CssVariableManager,
    pub base_offset: usize,
    pub inline: bool,
    pub usage_context_override: Option<&'a str>,
    pub dom_node: Option<DOMNodeInfo>,
}

/// Parse a CSS document and extract variable definitions and usages
pub async fn parse_css_document(
    text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
) -> Result<(), String> {
    let context = CssParseContext {
        css_text: text,
        full_text: text,
        uri,
        manager,
        base_offset: 0,
        inline: false,
        usage_context_override: None,
        dom_node: None,
    };
    parse_css_snippet(context).await
}

/// Parse a CSS snippet with a base offset into the full document.
pub async fn parse_css_snippet(context: CssParseContext<'_>) -> Result<(), String> {
    extract_definitions(
        context.css_text,
        context.full_text,
        context.uri,
        context.manager,
        context.base_offset,
        context.inline,
        context.usage_context_override,
    )
    .await;
    extract_usages(
        context.css_text,
        context.full_text,
        context.uri,
        context.manager,
        context.base_offset,
        context.usage_context_override,
        context.dom_node,
    )
    .await;
    extract_literal_colors(
        context.css_text,
        context.full_text,
        context.uri,
        context.manager,
        context.base_offset,
        context.usage_context_override,
    )
    .await;
    Ok(())
}

async fn extract_definitions(
    css_text: &str,
    full_text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
    base_offset: usize,
    inline: bool,
    selector_override: Option<&str>,
) {
    for_each_declaration(
        css_text,
        selector_override,
        |property_name,
         property_name_start,
         property_name_end,
         value_start,
         value_end,
         selector| {
            if !property_name.starts_with("--") {
                return None;
            }

            let value = css_text[value_start..value_end].trim().to_string();
            let abs_name_start = base_offset + property_name_start;
            let abs_name_end = base_offset + property_name_end;
            let abs_value_start = base_offset + value_start;
            let abs_value_end = base_offset + value_end;

            Some(CssVariable {
                name: property_name.to_string(),
                value: value.clone(),
                uri: uri.clone(),
                range: Range::new(
                    offset_to_position(full_text, abs_name_start),
                    offset_to_position(full_text, abs_value_end),
                ),
                name_range: Some(Range::new(
                    offset_to_position(full_text, abs_name_start),
                    offset_to_position(full_text, abs_name_end),
                )),
                value_range: Some(Range::new(
                    offset_to_position(full_text, abs_value_start),
                    offset_to_position(full_text, abs_value_end),
                )),
                selector,
                important: value.to_lowercase().contains("!important"),
                inline,
                source_position: abs_name_start,
            })
        },
        |variable| async move {
            if let Err(e) = manager.add_variable(variable).await {
                warn!("Failed to add CSS variable: {}", e);
            }
        },
    )
    .await;
}

async fn extract_literal_colors(
    css_text: &str,
    full_text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
    base_offset: usize,
    selector_override: Option<&str>,
) {
    for_each_declaration(
        css_text,
        selector_override,
        |_, _, _, value_start, value_end, selector| {
            let value = &css_text[value_start..value_end];
            let colors = extract_literal_colors_from_value(value)
                .into_iter()
                .map(
                    |(relative_start, relative_end, normalized_color)| LiteralColorOccurrence {
                        text: value[relative_start..relative_end].to_string(),
                        uri: uri.clone(),
                        range: Range::new(
                            offset_to_position(
                                full_text,
                                base_offset + value_start + relative_start,
                            ),
                            offset_to_position(full_text, base_offset + value_start + relative_end),
                        ),
                        usage_context: selector.clone(),
                        normalized_color,
                    },
                )
                .collect::<Vec<_>>();
            Some(colors)
        },
        |occurrences| async move {
            for occurrence in occurrences {
                manager.add_literal_color(occurrence).await;
            }
        },
    )
    .await;
}

async fn for_each_declaration<T, F, Fut>(
    css_text: &str,
    selector_override: Option<&str>,
    mut build: F,
    mut on_item: impl FnMut(T) -> Fut,
) where
    F: FnMut(&str, usize, usize, usize, usize, String) -> Option<T>,
    Fut: std::future::Future<Output = ()>,
{
    let bytes = css_text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut brace_depth = 0;
    let mut in_at_rule = false;
    let mut declaration_start = 0usize;
    let allow_without_braces = selector_override.is_some();

    while i < len {
        if in_comment {
            if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(quote) = in_string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_comment = true;
            i += 2;
            continue;
        }

        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        if bytes[i] == b'@' {
            in_at_rule = true;
        }

        if bytes[i] == b'{' {
            brace_depth += 1;
            if in_at_rule {
                in_at_rule = false;
            }
            declaration_start = i + 1;
            i += 1;
            continue;
        }

        if bytes[i] == b'}' {
            brace_depth -= 1;
            if brace_depth < 0 {
                brace_depth = 0;
            }
            declaration_start = i + 1;
            i += 1;
            continue;
        }

        if bytes[i] == b';' {
            declaration_start = i + 1;
            i += 1;
            continue;
        }

        if bytes[i] != b':' || (brace_depth == 0 && !allow_without_braces) {
            i += 1;
            continue;
        }

        let mut name_end = i;
        while name_end > declaration_start && bytes[name_end - 1].is_ascii_whitespace() {
            name_end -= 1;
        }

        let mut name_start = name_end;
        while name_start > declaration_start && is_ident_char(bytes[name_start - 1]) {
            name_start -= 1;
        }

        if name_end <= name_start {
            i += 1;
            continue;
        }

        let property_name = &css_text[name_start..name_end];
        let mut value_start = i + 1;
        while value_start < len && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }

        let mut value_end = value_start;
        let mut depth = 0i32;
        let mut val_in_comment = false;
        let mut val_in_string: Option<u8> = None;
        while value_end < len {
            let b = bytes[value_end];
            if val_in_comment {
                if value_end + 1 < len && b == b'*' && bytes[value_end + 1] == b'/' {
                    val_in_comment = false;
                    value_end += 2;
                    continue;
                }
                value_end += 1;
                continue;
            }
            if let Some(q) = val_in_string {
                if b == b'\\' {
                    value_end += 2;
                    continue;
                }
                if b == q {
                    val_in_string = None;
                }
                value_end += 1;
                continue;
            }
            if value_end + 1 < len && b == b'/' && bytes[value_end + 1] == b'*' {
                val_in_comment = true;
                value_end += 2;
                continue;
            }
            if b == b'"' || b == b'\'' {
                val_in_string = Some(b);
                value_end += 1;
                continue;
            }
            if b == b'(' {
                depth += 1;
                value_end += 1;
                continue;
            }
            if b == b')' && depth > 0 {
                depth -= 1;
                value_end += 1;
                continue;
            }
            if depth == 0 && (b == b';' || b == b'}') {
                break;
            }
            value_end += 1;
        }

        let mut value_end_trim = value_end;
        while value_end_trim > value_start && bytes[value_end_trim - 1].is_ascii_whitespace() {
            value_end_trim -= 1;
        }

        let selector = selector_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| find_selector_before(css_text, name_start, in_at_rule));

        if let Some(item) = build(
            property_name,
            name_start,
            name_end,
            value_start,
            value_end_trim,
            selector,
        ) {
            on_item(item).await;
        }

        i = value_end;
    }
}

async fn extract_usages(
    css_text: &str,
    full_text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
    base_offset: usize,
    usage_context_override: Option<&str>,
    dom_node: Option<DOMNodeInfo>,
) {
    let bytes = css_text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut brace_depth = 0;
    let mut in_at_rule = false;

    while i < len {
        if in_comment {
            if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(quote) = in_string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_comment = true;
            i += 2;
            continue;
        }

        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        // Track braces for scope
        if bytes[i] == b'{' {
            brace_depth += 1;
        } else if bytes[i] == b'}' {
            brace_depth -= 1;
            if brace_depth < 0 {
                brace_depth = 0;
            }
        }

        // Track @-rules
        if bytes[i] == b'@' && !in_comment && in_string.is_none() {
            in_at_rule = true;
        } else if bytes[i] == b'{' && in_at_rule {
            in_at_rule = false;
        }

        if is_var_function(bytes, i) {
            let var_start = i;
            let mut j = i + 3;
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= len || bytes[j] != b'(' {
                i += 1;
                continue;
            }
            let args_start = j + 1;
            let mut name_start = None;
            let mut name_end = None;
            let mut k = args_start;
            while k < len && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k + 1 < len && bytes[k] == b'-' && bytes[k + 1] == b'-' {
                name_start = Some(k);
                k += 2;
                while k < len && is_ident_char(bytes[k]) {
                    k += 1;
                }
                name_end = Some(k);
            }

            let mut depth = 1i32;
            let mut p = args_start;
            let mut var_in_comment = false;
            let mut var_in_string: Option<u8> = None;
            while p < len && depth > 0 {
                let b = bytes[p];
                if var_in_comment {
                    if p + 1 < len && b == b'*' && bytes[p + 1] == b'/' {
                        var_in_comment = false;
                        p += 2;
                        continue;
                    }
                    p += 1;
                    continue;
                }
                if let Some(q) = var_in_string {
                    if b == b'\\' {
                        p += 2;
                        continue;
                    }
                    if b == q {
                        var_in_string = None;
                    }
                    p += 1;
                    continue;
                }
                if p + 1 < len && b == b'/' && bytes[p + 1] == b'*' {
                    var_in_comment = true;
                    p += 2;
                    continue;
                }
                if b == b'"' || b == b'\'' {
                    var_in_string = Some(b);
                    p += 1;
                    continue;
                }
                if b == b'(' {
                    depth += 1;
                    p += 1;
                    continue;
                }
                if b == b')' {
                    depth -= 1;
                    p += 1;
                    continue;
                }
                p += 1;
            }

            let var_end = p.min(len);
            if let (Some(ns), Some(ne)) = (name_start, name_end) {
                let name = css_text[ns..ne].to_string();
                let usage_context = usage_context_override
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| find_selector_before(css_text, var_start, in_at_rule));
                let abs_start = base_offset + var_start;
                let abs_end = base_offset + var_end;
                let abs_name_start = base_offset + ns;
                let abs_name_end = base_offset + ne;

                let usage = CssVariableUsage {
                    name,
                    uri: uri.clone(),
                    range: Range::new(
                        offset_to_position(full_text, abs_start),
                        offset_to_position(full_text, abs_end),
                    ),
                    name_range: Some(Range::new(
                        offset_to_position(full_text, abs_name_start),
                        offset_to_position(full_text, abs_name_end),
                    )),
                    usage_context,
                    dom_node: dom_node.clone(),
                };
                manager.add_usage(usage).await;
            }

            i = var_end;
            continue;
        }

        i += 1;
    }
}

fn is_var_function(bytes: &[u8], idx: usize) -> bool {
    if idx + 2 >= bytes.len() {
        return false;
    }
    if !bytes[idx].eq_ignore_ascii_case(&b'v')
        || !bytes[idx + 1].eq_ignore_ascii_case(&b'a')
        || !bytes[idx + 2].eq_ignore_ascii_case(&b'r')
    {
        return false;
    }
    if idx > 0 && is_ident_char(bytes[idx - 1]) {
        return false;
    }
    true
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn extract_literal_colors_from_value(
    value: &str,
) -> Vec<(usize, usize, crate::color::NormalizedColorKey)> {
    let bytes = value.as_bytes();
    let ignored_ranges = find_ignored_var_ranges(value);
    let mut colors = Vec::new();
    let mut i = 0usize;
    let mut ignored_idx = 0usize;
    let mut in_string: Option<u8> = None;

    while i < bytes.len() {
        while ignored_idx < ignored_ranges.len() && i >= ignored_ranges[ignored_idx].1 {
            ignored_idx += 1;
        }
        if ignored_idx < ignored_ranges.len() && i >= ignored_ranges[ignored_idx].0 {
            i = ignored_ranges[ignored_idx].1;
            continue;
        }

        if let Some(quote) = in_string {
            if bytes[i] == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        if bytes[i] == b'#' {
            let mut end = i + 1;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                end += 1;
            }
            let len = end - i;
            if matches!(len, 3..=9) {
                if let Some(color) = normalized_color_key(&value[i..end]) {
                    colors.push((i, end, color));
                }
            }
            i = end;
            continue;
        }

        if bytes[i].is_ascii_alphabetic() {
            let start = i;
            let mut end = i + 1;
            while end < bytes.len() && is_ident_char(bytes[end]) {
                end += 1;
            }

            let mut j = end;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }

            if j < bytes.len() && bytes[j] == b'(' {
                let ident = value[start..end].to_ascii_lowercase();
                if matches!(ident.as_str(), "rgb" | "rgba" | "hsl" | "hsla") {
                    if let Some(func_end) = find_balanced_call_end(value, j) {
                        if let Some(color) = normalized_color_key(&value[start..func_end]) {
                            colors.push((start, func_end, color));
                        }
                        i = func_end;
                        continue;
                    }
                }
            } else if let Some(color) = normalized_color_key(&value[start..end]) {
                colors.push((start, end, color));
            }

            i = end;
            continue;
        }

        i += 1;
    }

    colors
}

fn find_ignored_var_ranges(value: &str) -> Vec<(usize, usize)> {
    let bytes = value.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0usize;
    let mut in_string: Option<u8> = None;

    while i < bytes.len() {
        if let Some(quote) = in_string {
            if bytes[i] == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if bytes[i] == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        if is_var_function(bytes, i) {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                if let Some(end) = find_balanced_call_end(value, j) {
                    ranges.push((i, end));
                    i = end;
                    continue;
                }
            }
        }

        i += 1;
    }

    ranges
}

fn find_balanced_call_end(value: &str, open_paren_idx: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut depth = 0i32;
    let mut i = open_paren_idx;
    let mut in_string: Option<u8> = None;

    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_string {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == q {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if b == b'"' || b == b'\'' {
            in_string = Some(b);
            i += 1;
            continue;
        }

        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }

    None
}

fn find_selector_before(text: &str, offset: usize, in_at_rule: bool) -> String {
    let before = &text[..offset];

    if in_at_rule {
        // For variables defined in @-rules, find the @-rule context
        if let Some(at_pos) = before.rfind('@') {
            let at_rule_end = before[at_pos..]
                .find('{')
                .map(|pos| pos + at_pos)
                .unwrap_or(before.len());
            let at_rule = before[at_pos..at_rule_end].trim();
            return format!("@{}", at_rule);
        }
        return "@unknown".to_string();
    }

    if let Some(brace_pos) = before.rfind('{') {
        let start = before[..brace_pos].rfind('}').map(|p| p + 1).unwrap_or(0);
        let selector_block = before[start..brace_pos].trim();

        // Handle complex selectors that might span multiple lines or have nested braces
        let selector = extract_last_selector(selector_block);

        if selector.is_empty() {
            ":root".to_string()
        } else {
            selector
        }
    } else {
        ":root".to_string()
    }
}

/// Extract the last selector from a selector block, handling complex cases
fn extract_last_selector(selector_block: &str) -> String {
    // Find the last complete selector by tracking balanced parentheses and commas
    let bytes = selector_block.as_bytes();
    let len = bytes.len();
    let mut paren_depth: usize = 0;
    let mut last_selector_start = 0;
    let last_selector_end = len;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => {
                paren_depth += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            b',' if paren_depth == 0 => {
                // This is a selector list separator
                // The next character (if any) starts a new selector
                last_selector_start = i + 1;
            }
            _ => {}
        }
    }

    // Extract the last selector
    let last_selector = selector_block[last_selector_start..last_selector_end].trim();

    // Clean up the selector - remove any trailing braces or CSS at-rules
    let cleaned = last_selector
        .split('{')
        .next()
        .unwrap_or(last_selector)
        .trim();

    // Handle CSS at-rules by finding the actual selector part
    let selector = if cleaned.starts_with('@') {
        // This is an at-rule like @media, find the selector inside
        if let Some(open_brace) = cleaned.find('{') {
            cleaned[..open_brace].trim().to_string()
        } else {
            cleaned.to_string()
        }
    } else {
        cleaned.to_string()
    };

    if selector.is_empty() {
        ":root".to_string()
    } else {
        selector
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::CssVariableManager;
    use crate::types::Config;
    use std::collections::HashSet;
    use std::str::FromStr;

    #[tokio::test]
    async fn parse_css_document_extracts_definitions_and_usages() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let text = ":root { --primary: #fff; color: var(--primary); } \
                    .button { --secondary: var(--primary, #000); }";

        parse_css_document(text, &uri, &manager).await.unwrap();

        let primary_defs = manager.get_variables("--primary").await;
        assert_eq!(primary_defs.len(), 1);
        assert_eq!(primary_defs[0].value, "#fff");

        let secondary_defs = manager.get_variables("--secondary").await;
        assert_eq!(secondary_defs.len(), 1);
        assert_eq!(secondary_defs[0].value, "var(--primary, #000)");

        let usages = manager.get_usages("--primary").await;
        assert_eq!(usages.len(), 2);

        let contexts: HashSet<String> = usages.into_iter().map(|u| u.usage_context).collect();
        assert!(contexts.contains(":root"));
        assert!(contexts.contains(".button"));
    }

    #[tokio::test]
    async fn parse_css_document_skips_nested_var_fallback_usages() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let text = ".button { color: var(--primary, var(--fallback)); }";

        parse_css_document(text, &uri, &manager).await.unwrap();

        let primary_usages = manager.get_usages("--primary").await;
        assert_eq!(primary_usages.len(), 1);

        let fallback_usages = manager.get_usages("--fallback").await;
        assert_eq!(fallback_usages.len(), 0);
    }

    #[tokio::test]
    async fn parse_css_document_extracts_literal_colors_in_compound_values() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let text = r#"
            .button {
                color: #fff;
                background: linear-gradient(red, rgb(255 255 255));
                box-shadow: 0 0 4px rgba(0, 0, 0, 0.5);
            }
        "#;

        parse_css_document(text, &uri, &manager).await.unwrap();

        let occurrences = manager.get_document_literal_colors(&uri).await;
        let literals: HashSet<String> = occurrences.into_iter().map(|occ| occ.text).collect();
        assert!(literals.contains("#fff"));
        assert!(literals.contains("red"));
        assert!(literals.contains("rgb(255 255 255)"));
        assert!(literals.contains("rgba(0, 0, 0, 0.5)"));
    }

    #[tokio::test]
    async fn parse_css_document_ignores_literal_colors_inside_var_calls() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let text = r#"
            .button {
                color: var(--primary, #fff);
                background: linear-gradient(var(--from, red), blue);
            }
        "#;

        parse_css_document(text, &uri, &manager).await.unwrap();

        let occurrences = manager.get_document_literal_colors(&uri).await;
        let literals: HashSet<String> = occurrences.into_iter().map(|occ| occ.text).collect();
        assert!(!literals.contains("#fff"));
        assert!(!literals.contains("red"));
        assert!(literals.contains("blue"));
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use crate::types::Config;
    use ls_types::Uri;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_parse_empty_css() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///empty.css").unwrap();

        let result = parse_css_document("", &uri, &manager).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parse_css_with_comments() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            /* Comment before */
            :root {
                /* Inline comment */
                --primary: blue; /* End comment */
                --secondary: red;
            }
            /* Comment after */
        "#;

        let result = parse_css_document(css, &uri, &manager).await;
        assert!(result.is_ok());

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_css_with_important() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root {
                --color: red !important;
                --spacing: 1rem;
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_variables("--color").await;
        assert_eq!(vars.len(), 1);
        assert!(vars[0].important);

        let spacing = manager.get_variables("--spacing").await;
        assert!(!spacing[0].important);
    }

    #[tokio::test]
    async fn test_parse_css_var_with_fallback() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            .button {
                color: var(--primary, blue);
                background: var(--bg, var(--fallback, #fff));
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let primary_usages = manager.get_usages("--primary").await;
        assert_eq!(primary_usages.len(), 1);
        // Fallback values are parsed but not stored in the usage struct

        let bg_usages = manager.get_usages("--bg").await;
        assert_eq!(bg_usages.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_css_complex_selectors() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            #id .class > div[data-attr="value"]:hover::before {
                --complex: value;
            }
            
            @media (min-width: 768px) {
                .responsive {
                    --media: query;
                }
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert!(vars.len() >= 2);
    }

    #[tokio::test]
    async fn test_parse_css_multiline_values() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root {
                --gradient: linear-gradient(
                    to bottom,
                    red,
                    blue
                );
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_variables("--gradient").await;
        assert_eq!(vars.len(), 1);
        assert!(vars[0].value.contains("linear-gradient"));
    }

    #[tokio::test]
    async fn test_parse_css_variable_names_with_dashes() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root {
                --primary-color: blue;
                --bg-color-dark: #333;
                --font-size-xl: 2rem;
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 3);
        assert!(vars.iter().any(|v| v.name == "--primary-color"));
        assert!(vars.iter().any(|v| v.name == "--bg-color-dark"));
        assert!(vars.iter().any(|v| v.name == "--font-size-xl"));
    }

    #[tokio::test]
    async fn test_parse_css_special_characters_in_values() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root {
                --shadow: 0 2px 4px rgba(0,0,0,0.1);
                --calc: calc(100% - 20px);
                --url: url("https://example.com/image.jpg");
                --content: "Hello, World!";
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 4);
    }

    #[tokio::test]
    async fn test_parse_css_nested_var_calls() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            .element {
                color: var(--primary);
                background: var(--bg);
                border: 1px solid var(--border-color);
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        assert_eq!(manager.get_usages("--primary").await.len(), 1);
        assert_eq!(manager.get_usages("--bg").await.len(), 1);
        assert_eq!(manager.get_usages("--border-color").await.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_css_whitespace_variations() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root{--no-space:value;}
            :root { --normal-space: value; }
            :root  {  --extra-space  :  value  ;  }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let vars = manager.get_all_variables().await;
        assert_eq!(vars.len(), 3);
    }

    #[tokio::test]
    async fn test_parse_css_malformed_but_parseable() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        // Missing closing brace, but should still parse what it can
        let css = r#"
            :root {
                --valid: blue;
        "#;

        let result = parse_css_document(css, &uri, &manager).await;
        assert!(result.is_ok());
    }

    /// Bug demonstration: Complex pseudo-selectors are not parsed correctly
    ///
    /// ISSUE: The extract_last_selector function may have issues with:
    /// - Complex pseudo-selectors like :nth-child(2n+1)
    /// - Attribute selectors with complex values
    /// - Nested parentheses
    ///
    /// EXPECTED TO FAIL: This test proves edge cases are not handled.
    /// After fix: Complex selectors should be extracted correctly.
    #[test]
    fn test_extract_last_selector_complex_pseudo() {
        use crate::specificity::calculate_specificity;

        let test_cases = vec![
            // (input, expected selector that should be present)
            (":root", "root"),
            (":host", "host"),
            (".class", "class"),
            ("#id", "id"),
            ("div.class", "div.class"),
            ("div::before", "div::before"),
            // Complex pseudo-selectors that may fail
            (":nth-child(2n)", "nth-child"),
            (":nth-child(2n+1)", "nth-child"),
            (":nth-child(odd)", "nth-child"),
            (":nth-child(3n-1)", "nth-child"),
            (":nth-of-type(2n)", "nth-of-type"),
            (":not(.hidden)", "not"),
            (":is(div, span)", "is"),
            (":where(.theme)", "where"),
            (":has(+ div)", "has"),
            (":first-letter", "first-letter"),
            (":first-line", "first-line"),
            (":placeholder-shown", "placeholder-shown"),
            (":focus-visible", "focus-visible"),
            (":focus-within", "focus-within"),
            // Complex attribute selectors
            ("[data-value^=\"test\"]", "data-value"),
            ("[class~=\"token\"]", "class"),
            ("[lang|=\"en\"]", "lang"),
        ];

        for (input, expected_contains) in test_cases {
            // Find selector before a position (simulating cursor at end)
            let css = format!("{} {{ color: red; }}", input);
            let position = css.len() - 1; // Position after selector

            let result = find_selector_before(&css, position, false);

            assert!(
                result.contains(expected_contains),
                "Selector '{}' should contain '{}' (from input: {})",
                result,
                expected_contains,
                input
            );

            // Also verify specificity calculation doesn't panic
            let specificity = calculate_specificity(&result);

            // For complex selectors, specificity should still be calculable
            let _ = specificity; // verify calculate_specificity doesn't panic
        }

        // Additional edge case: selector with nested pseudo-classes
        let nested = ".container:not(:has(.hidden)):nth-child(2n+1)";
        let result = find_selector_before(
            &format!("{} {{ color: red; }}", nested),
            nested.len() + 5,
            false,
        );

        // BUG: Currently this assertion may FAIL because nested selectors are not handled
        // After fix: Should extract the full compound selector
        assert!(
            result.contains("container") && result.contains("not") && result.contains("nth-child"),
            "Nested selector '{}' should contain all parts, got: {}",
            nested,
            result
        );
    }
}
