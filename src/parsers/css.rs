use ls_types::{Range, Uri};
use tracing::warn;

use crate::color::normalized_color_key;
use crate::manager::CssVariableManager;
use crate::types::{
    offset_to_position, CssVariable, CssVariableUsage, DOMNodeInfo, LiteralColorOccurrence,
};

const UNKNOWN_SELECTOR: &str = "<unknown>";

/// Maximum input size to prevent memory exhaustion (10MB)
const MAX_INPUT_SIZE_BYTES: usize = 10 * 1024 * 1024;

/// At-rules that block variable extraction (descriptors, not CSS properties)
const BLOCK_LIST: &[&str] = &[
    "@font-face",
    "@property",
    "@keyframes",
    "@counter-style",
    "@font-feature-values",
    "@scroll-timeline",
];

/// Extract at-rule name, returns lowercase for case-insensitive matching.
/// Handles vendor prefixes and whitespace between @rule and {.
fn extract_at_rule_name(bytes: &[u8], start: usize) -> Option<String> {
    let remaining = &bytes[start + 1..]; // Skip '@'
    let mut end = 0;
    let mut found_ident = false;

    while end < remaining.len() {
        let b = remaining[end];
        if b.is_ascii_whitespace() {
            if found_ident {
                break; // Whitespace after ident = end of name
            }
            end += 1;
            continue;
        }
        if is_ident_char(b) || b == b'-' {
            found_ident = true;
            end += 1;
            continue;
        }
        break; // Non-ident char
    }

    if end > 0 {
        let name = std::str::from_utf8(&remaining[..end]).ok()?;
        Some(format!("@{}", name.to_ascii_lowercase()))
    } else {
        None
    }
}

/// Check if character is valid in CSS identifiers
#[inline]
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Returns true if this at-rule blocks custom property extraction.
/// Case-insensitive matching.
fn should_block_variables(at_rule: &str) -> bool {
    let at_rule_lower = at_rule.to_ascii_lowercase();

    // Check blocklist - handles @font-face, @property, @keyframes, etc.
    if BLOCK_LIST.iter().any(|name| *name == at_rule_lower) {
        return true;
    }

    // FIXED v3: Use contains("keyframes") instead of starts_with("@-")
    // because standard @keyframes doesn't start with "@-"
    if at_rule_lower.contains("keyframes") {
        return true;
    }

    false
}

fn trim_css_trivia_end(value: &str, mut end: usize) -> usize {
    loop {
        while end > 0 && value.as_bytes()[end - 1].is_ascii_whitespace() {
            end -= 1;
        }

        let Some(prefix) = value.get(..end) else {
            break;
        };
        if !prefix.ends_with("*/") {
            break;
        }
        let Some(comment_start) = prefix[..prefix.len() - 2].rfind("/*") else {
            break;
        };
        end = comment_start;
    }

    end
}

/// Split a trailing CSS `!important` annotation from a custom-property value.
///
/// CSS removes the annotation from the property's value while retaining its
/// cascade importance. Whitespace and comments are allowed around the `!`.
fn split_important_annotation(value: &str) -> (&str, bool) {
    let keyword_end = trim_css_trivia_end(value, value.len());
    let Some(keyword_start) = keyword_end.checked_sub("important".len()) else {
        return (value, false);
    };
    let Some(keyword) = value.get(keyword_start..keyword_end) else {
        return (value, false);
    };
    if !keyword.eq_ignore_ascii_case("important") {
        return (value, false);
    }

    let bang_end = trim_css_trivia_end(value, keyword_start);
    if bang_end == 0 || value.as_bytes()[bang_end - 1] != b'!' {
        return (value, false);
    }

    let value_end = trim_css_trivia_end(value, bang_end - 1);
    (&value[..value_end], true)
}

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
    // Memory bounds check to prevent exhaustion attacks
    if text.len() > MAX_INPUT_SIZE_BYTES {
        return Err(format!(
            "CSS input too large ({} bytes), maximum allowed is {} bytes",
            text.len(),
            MAX_INPUT_SIZE_BYTES
        ));
    }

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

            let raw_value = &css_text[value_start..value_end];
            let (value, important) = split_important_annotation(raw_value);
            let abs_name_start = base_offset + property_name_start;
            let abs_name_end = base_offset + property_name_end;
            let abs_value_start = base_offset + value_start;
            let abs_value_end = base_offset + value_end;
            let abs_semantic_value_end = abs_value_start + value.len();

            Some(CssVariable {
                name: property_name.to_string(),
                value: value.to_string(),
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
                    offset_to_position(full_text, abs_semantic_value_end),
                )),
                selector,
                important,
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
    let mut current_at_rule: Option<String> = None;
    let mut blocking_at_rule: Option<String> = None;
    let mut declaration_start = 0usize;
    let mut selector_stack: Vec<String> = Vec::with_capacity(16);
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

        if bytes[i] == b'@' && !in_comment && in_string.is_none() {
            // Extract at-rule name for case-insensitive matching
            current_at_rule = extract_at_rule_name(bytes, i);
        }

        if bytes[i] == b'{' {
            brace_depth += 1;

            // Check if this at-rule blocks variable extraction
            if let Some(ref at_rule) = current_at_rule {
                if should_block_variables(at_rule) {
                    blocking_at_rule = Some(at_rule.clone());
                }
            }

            // Skip selector push for blocking at-rules (@font-face, @keyframes, etc.)
            if blocking_at_rule.is_none() {
                selector_stack.push(resolve_block_selector(
                    css_text,
                    i,
                    current_at_rule.is_some(),
                    selector_stack.last(),
                ));
            }

            // Reset at-rule tracking after entering block
            current_at_rule = None;
            declaration_start = i + 1;
            i += 1;
            continue;
        }

        if bytes[i] == b'}' {
            brace_depth -= 1;
            if brace_depth < 0 {
                brace_depth = 0;
            }
            // SECURE: Guard against empty stack on malformed CSS
            if !selector_stack.is_empty() {
                selector_stack.pop();
            }
            // Clear blocking state when exiting a blocking at-rule's block
            if blocking_at_rule.is_some() && brace_depth == 0 {
                blocking_at_rule = None;
            }
            declaration_start = i + 1;
            i += 1;
            continue;
        }

        if bytes[i] == b';' {
            if current_at_rule.is_some() {
                // At-rule ended without braces (e.g., @import "file.css";)
                current_at_rule = None;
            }
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

        if has_non_whitespace_outside_comments(&css_text[declaration_start..name_start]) {
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
            .or_else(|| selector_stack.last().cloned())
            .or_else(|| find_selector_before(css_text, name_start, current_at_rule.is_some()))
            .unwrap_or_else(|| UNKNOWN_SELECTOR.to_string());

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
    let mut current_at_rule: Option<String> = None;
    let mut blocking_at_rule: Option<String> = None;
    let mut selector_stack: Vec<String> = Vec::with_capacity(16);

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
            // Check if this at-rule blocks variable extraction
            if let Some(ref at_rule) = current_at_rule {
                if should_block_variables(at_rule) {
                    blocking_at_rule = Some(at_rule.clone());
                }
            }

            // Skip selector push for blocking at-rules
            if blocking_at_rule.is_none() {
                selector_stack.push(resolve_block_selector(
                    css_text,
                    i,
                    current_at_rule.is_some(),
                    selector_stack.last(),
                ));
            }

            // Reset at-rule tracking after entering block
            current_at_rule = None;
            brace_depth += 1;
        } else if bytes[i] == b'}' {
            brace_depth -= 1;
            if brace_depth < 0 {
                brace_depth = 0;
            }
            // SECURE: Guard against empty stack on malformed CSS
            if !selector_stack.is_empty() {
                selector_stack.pop();
            }
            // Clear blocking state when exiting a blocking at-rule's block
            if blocking_at_rule.is_some() && brace_depth == 0 {
                blocking_at_rule = None;
            }
        }

        // Track @-rules
        if bytes[i] == b'@' && !in_comment && in_string.is_none() {
            current_at_rule = extract_at_rule_name(bytes, i);
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
                    .or_else(|| selector_stack.last().cloned())
                    .or_else(|| {
                        find_selector_before(css_text, var_start, current_at_rule.is_some())
                    })
                    .unwrap_or_else(|| UNKNOWN_SELECTOR.to_string());
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

            // Continue through the arguments so nested var() calls used as fallbacks are
            // indexed as usages too. Starting after the opening parenthesis avoids
            // re-indexing the outer call while retaining the normal comment/string guards.
            i = args_start;
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

fn has_non_whitespace_outside_comments(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut i = 0usize;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;

    while i < bytes.len() {
        if in_comment {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
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

        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_comment = true;
            i += 2;
            continue;
        }

        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        if !bytes[i].is_ascii_whitespace() {
            return true;
        }

        i += 1;
    }

    false
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

fn resolve_block_selector(
    text: &str,
    brace_pos: usize,
    in_at_rule: bool,
    parent_selector: Option<&String>,
) -> String {
    if in_at_rule {
        return parent_selector
            .cloned()
            .or_else(|| find_selector_before(text, brace_pos, true))
            .unwrap_or_else(|| UNKNOWN_SELECTOR.to_string());
    }

    let before = &text[..brace_pos];
    let start = before
        .rfind(['{', '}', ';'])
        .map(|pos| pos + 1)
        .unwrap_or(0);
    extract_last_selector(before[start..].trim()).unwrap_or_else(|| UNKNOWN_SELECTOR.to_string())
}

fn find_selector_before(text: &str, offset: usize, in_at_rule: bool) -> Option<String> {
    let before = &text[..offset];

    if in_at_rule {
        // For variables defined in @-rules, find the @-rule context
        if let Some(at_pos) = before.rfind('@') {
            let at_rule_end = before[at_pos..]
                .find('{')
                .map(|pos| pos + at_pos)
                .unwrap_or(before.len());
            let at_rule = before[at_pos..at_rule_end].trim();
            return Some(format!("@{}", at_rule));
        }
        return None;
    }

    if let Some(brace_pos) = before.rfind('{') {
        let start = before[..brace_pos].rfind('}').map(|p| p + 1).unwrap_or(0);
        let selector_block = before[start..brace_pos].trim();

        // If the selector block contains a nested `{` (from an @-rule), the actual
        // selector lives between the innermost `{` and the outer `{`.
        // e.g. "@media (min-width: 768px) { .responsive" → ".responsive"
        let inner_brace = before[start..brace_pos].rfind('{');
        let effective_block = if let Some(pos) = inner_brace {
            before[start + pos + 1..brace_pos].trim()
        } else {
            selector_block
        };

        // Handle complex selectors that might span multiple lines or have nested braces
        extract_last_selector(effective_block)
    } else {
        None
    }
}

/// Extract the last selector from a selector block, handling complex cases
fn extract_last_selector(selector_block: &str) -> Option<String> {
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
        None
    } else {
        Some(selector)
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
    async fn parse_css_document_indexes_nested_var_fallback_usages() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();
        let text = ".button { color: var(--primary, var(--fallback, var(--deep))); }";

        parse_css_document(text, &uri, &manager).await.unwrap();

        let primary_usages = manager.get_usages("--primary").await;
        assert_eq!(primary_usages.len(), 1);

        let fallback_usages = manager.get_usages("--fallback").await;
        assert_eq!(fallback_usages.len(), 1);
        let fallback_start = text.find("var(--fallback").unwrap();
        assert_eq!(
            fallback_usages[0].range.start,
            offset_to_position(text, fallback_start),
        );

        let deep_usages = manager.get_usages("--deep").await;
        assert_eq!(deep_usages.len(), 1);
        let deep_name_start = text.find("--deep").unwrap();
        assert_eq!(
            deep_usages[0].name_range.unwrap().start,
            offset_to_position(text, deep_name_start),
        );
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

    #[test]
    fn test_split_important_annotation_handles_css_trivia() {
        assert_eq!(split_important_annotation("red !important"), ("red", true));
        assert_eq!(
            split_important_annotation("#00f ! IMPORTANT /* trailing */"),
            ("#00f", true)
        );
        assert_eq!(
            split_important_annotation("rgb(0 0 0) !/**/important"),
            ("rgb(0 0 0)", true)
        );
        assert_eq!(
            split_important_annotation("red /* !important */"),
            ("red /* !important */", false)
        );
        assert_eq!(
            split_important_annotation("red!important-value"),
            ("red!important-value", false)
        );
    }

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
        assert_eq!(vars[0].value, "red");

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
    async fn test_parse_css_variables_after_nested_media_inside_root() {
        let manager = CssVariableManager::new(Config::default());
        let uri = Uri::from_str("file:///test.css").unwrap();

        let css = r#"
            :root {
                --before: blue;

                @media (prefers-color-scheme: dark) {
                    --during: red;
                }

                --after: green;
            }
        "#;

        parse_css_document(css, &uri, &manager).await.unwrap();

        let before = manager.get_variables("--before").await;
        let during = manager.get_variables("--during").await;
        let after = manager.get_variables("--after").await;

        assert_eq!(before.len(), 1);
        assert_eq!(during.len(), 1);
        assert_eq!(after.len(), 1);
        assert_eq!(before[0].selector, ":root");
        assert_eq!(during[0].selector, ":root");
        assert_eq!(after[0].selector, ":root");
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

    #[test]
    fn test_find_selector_in_at_rule_block() {
        // Bug: @-rule prelude is returned instead of the actual selector
        let css = "@media (min-width: 768px) { .responsive { color: var(--x); } }";
        let var_pos = css.find("var").unwrap();
        let result = find_selector_before(css, var_pos, false);
        assert_eq!(
            result,
            Some(".responsive".to_string()),
            "Expected selector '.responsive' inside @media block, got: '{}'",
            result.as_deref().unwrap_or("<none>")
        );
    }

    #[test]
    fn test_find_selector_deeply_nested_at_rule() {
        let css = "@media (min-width: 768px) { @supports (display: grid) { .grid-item { color: var(--x); } } }";
        let var_pos = css.find("var").unwrap();
        let result = find_selector_before(css, var_pos, false);
        assert_eq!(
            result,
            Some(".grid-item".to_string()),
            "Expected selector '.grid-item' inside nested @-rules, got: '{}'",
            result.as_deref().unwrap_or("<none>")
        );
    }

    #[test]
    fn test_find_selector_definition_in_at_rule() {
        let css = "@media (min-width: 768px) { .responsive { --responsive: value; } }";
        let decl_pos = css.find("--responsive").unwrap();
        let result = find_selector_before(css, decl_pos, false);
        assert_eq!(
            result,
            Some(".responsive".to_string()),
            "Expected selector '.responsive' for definition inside @media, got: '{}'",
            result.as_deref().unwrap_or("<none>")
        );
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
            let result = result.expect("selector should be present");

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
        )
        .expect("selector should be present");

        // BUG: Currently this assertion may FAIL because nested selectors are not handled
        // After fix: Should extract the full compound selector
        assert!(
            result.contains("container") && result.contains("not") && result.contains("nth-child"),
            "Nested selector '{}' should contain all parts, got: {}",
            nested,
            result
        );
    }

    #[test]
    fn test_find_selector_before_returns_none_without_selector_context() {
        assert_eq!(find_selector_before("--x: red;", 4, false), None);
    }
}
