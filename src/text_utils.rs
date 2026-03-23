use crate::types::{offset_to_position, position_to_offset, CssVariable};
use ls_types::{Position, Range, TextDocumentContentChangeEvent};

pub fn clamp_to_char_boundary(text: &str, mut idx: usize) -> usize {
    if idx > text.len() {
        idx = text.len();
    }
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

pub fn is_word_byte(b: u8) -> bool {
    is_word_char(b as char)
}

pub fn range_contains_position(range: &Range, position: Position) -> bool {
    range.start <= position && position <= range.end
}

/// Check if `outer` range completely contains `inner` range
pub fn range_contains(outer: &Range, inner: &Range) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub fn apply_change_to_text(text: &mut String, change: &TextDocumentContentChangeEvent) {
    if let Some(range) = change.range {
        let start = position_to_offset(text, range.start);
        let end = position_to_offset(text, range.end);
        if let (Some(start), Some(end)) = (start, end) {
            if start <= end && end <= text.len() {
                text.replace_range(start..end, &change.text);
                return;
            }
        }
    }
    *text = change.text.clone();
}

pub fn find_value_range_in_definition(text: &str, def: &CssVariable) -> Option<Range> {
    let start = position_to_offset(text, def.range.start)?;
    let end = position_to_offset(text, def.range.end)?;
    if start >= end || end > text.len() {
        return None;
    }
    let def_text = &text[start..end];
    let colon_index = def_text.find(':')?;
    let after_colon = &def_text[colon_index + 1..];
    let value_trim = def.value.trim();
    let value_index = after_colon.find(value_trim)?;

    let absolute_start = start + colon_index + 1 + value_index;
    let absolute_end = absolute_start + value_trim.len();

    Some(Range::new(
        offset_to_position(text, absolute_start),
        offset_to_position(text, absolute_end),
    ))
}
