//! Regression guards for css-variables-zed issue #16.
//!
//! Ensures workspace-wide `var(--` completion context is detected in long CSS
//! rule blocks, not only within a 400-character lookback window.
//!
//! Run with: cargo test --test issue_16_guard_test

use css_variable_lsp::completion_context::{
    completion_value_context_slice, get_value_context_info,
};
use css_variable_lsp::document_kind::build_lookup_extension_map;
use css_variable_lsp::types::{offset_to_position, Config};
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
fn issue_16_fixed_long_rule_detects_value_context_at_bottom() {
    let text = build_long_css_rule();
    assert!(
        text.len() > 500,
        "fixture must exceed the old 400-char lookback window (len={})",
        text.len()
    );

    let lookup_map = build_lookup_extension_map(&Config::default().lookup_files);
    let uri = Uri::from_str("file:///styles.css").unwrap();
    let position = offset_to_position(&text, text.len());

    let context = completion_value_context_slice(&text, position, None, &uri, &lookup_map)
        .expect("css document should yield a completion slice");
    let value_context = get_value_context_info(context.slice, context.allow_without_braces);

    assert!(
        value_context.is_value_context,
        "var(-- at the bottom of a long rule must be in value context"
    );
    assert_eq!(value_context.property_name.as_deref(), Some("color"));
}

#[test]
fn issue_16_fixed_short_rule_still_works() {
    let text = ".card { color: var(--";
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
fn issue_16_fixed_top_of_class_still_works() {
    let text = ".card {\n  color: var(--";
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
fn issue_16_fixed_nested_rule_detects_inner_property() {
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
