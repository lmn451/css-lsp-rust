use once_cell::sync::Lazy;
use regex::Regex;

use crate::dom_tree::DomTree;
use crate::types::{CssVariable, DOMNodeInfo};

/// Memoized regex patterns for specificity calculation
static PSEUDO_ELEMENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"::[a-zA-Z-]+").unwrap());
static ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"#[a-zA-Z0-9_-]+").unwrap());
static CLASS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.[a-zA-Z0-9_-]+").unwrap());
static ATTR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\[[^\]'"']*\]"#).unwrap());
static NOT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r":not\((?:[^()]|\([^)]*\))+\)").unwrap());
static IS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r":is\((?:[^()]|\([^)]*\))+\)").unwrap());
static WHERE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r":where\((?:[^()]|\([^)]*\))+\)").unwrap());
static PSEUDO_CLASS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r":[a-zA-Z-]+(\([^)]*\))?").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Specificity {
    pub ids: u32,
    pub classes: u32,
    pub elements: u32,
}

impl Specificity {
    pub fn new(ids: u32, classes: u32, elements: u32) -> Self {
        Self {
            ids,
            classes,
            elements,
        }
    }
}

/// Extract the argument of a top-level :is() call.
/// Uses a depth tracker to handle nested parentheses correctly.
fn extract_is_arg(selector: &str) -> Option<&str> {
    let bytes = selector.as_bytes();
    let len = bytes.len();

    let is_prefix = b":is(";
    let is_len = is_prefix.len();

    for i in 0..len {
        if bytes[i] != b':' || i + is_len > len {
            continue;
        }
        let mut match_is = true;
        for j in 0..is_len {
            if bytes[i + j] != is_prefix[j] {
                match_is = false;
                break;
            }
        }
        if !match_is {
            continue;
        }

        // Found :is( at position i. Now find matching ) at depth 1.
        let mut depth = 1;
        let mut pos = i + is_len;
        while pos < len {
            match bytes[pos] {
                b'(' => {
                    depth += 1;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&selector[i + is_len..pos]);
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        return None;
    }
    None
}

/// Extract the argument of a top-level :not() call.
/// Uses a depth tracker to handle nested parentheses correctly.
fn extract_not_arg(selector: &str) -> Option<&str> {
    let bytes = selector.as_bytes();
    let len = bytes.len();

    let not_prefix = b":not(";
    let not_len = not_prefix.len();

    for i in 0..len {
        if bytes[i] != b':' || i + not_len > len {
            continue;
        }
        let mut match_not = true;
        for j in 0..not_len {
            if bytes[i + j] != not_prefix[j] {
                match_not = false;
                break;
            }
        }
        if !match_not {
            continue;
        }

        // Found :not( at position i. Now find matching ) at depth 1.
        let mut depth = 1;
        let mut pos = i + not_len;
        while pos < len {
            match bytes[pos] {
                b'(' => {
                    depth += 1;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&selector[i + not_len..pos]);
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        return None;
    }
    None
}

/// Calculate the full specificity of a :not(), :is(), :where() argument.
/// Per CSS spec: :not(.foo, #bar) → take max IDs=1, max classes=1.
fn specificity_of_not_arg(arg: &str) -> Specificity {
    let parts: Vec<&str> = arg
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Specificity::new(0, 0, 0);
    }
    let mut max_ids: u32 = 0;
    let mut max_classes: u32 = 0;
    let mut max_elements: u32 = 0;
    for part in parts {
        let spec = specificity_of_selector_part(part);
        max_ids = max_ids.max(spec.ids);
        max_classes = max_classes.max(spec.classes);
        max_elements = max_elements.max(spec.elements);
    }
    Specificity::new(max_ids, max_classes, max_elements)
}

/// Calculate specificity for a single selector part (no comma-separated handling).
/// Handles nested :not(), :is(), :where() by extracting and processing their arguments.
fn specificity_of_selector_part(selector: &str) -> Specificity {
    let selector = selector.trim();
    if selector.is_empty() || selector == "*" {
        return Specificity::new(0, 0, 0);
    }
    let mut working = selector.to_string();

    let pseudo_elements = PSEUDO_ELEMENT_RE.find_iter(&working).count() as u32;
    working = PSEUDO_ELEMENT_RE.replace_all(&working, "").to_string();

    // Recursively handle nested :not() in this argument.
    let not_arg = extract_not_arg(selector);
    let (extra_ids, extra_classes, extra_elements) = if let Some(arg) = not_arg {
        let spec = specificity_of_not_arg(arg);
        (spec.ids, spec.classes, spec.elements)
    } else {
        (0, 0, 0)
    };
    // Remove :not() blocks
    working = NOT_RE.replace_all(&working, "").to_string();

    // Recursively handle nested :is() in this argument.
    // :is() adds specificity of its argument per CSS spec.
    let is_arg = extract_is_arg(selector);
    let (is_ids, is_classes, is_elements) = if let Some(arg) = is_arg {
        let spec = specificity_of_not_arg(arg);
        (spec.ids, spec.classes, spec.elements)
    } else {
        (0, 0, 0)
    };
    // Remove :is() blocks so they aren't counted as pseudo-classes
    working = IS_RE.replace_all(&working, "").to_string();

    // :where() has zero specificity per CSS spec - it's transparent.
    // Remove :where() blocks without processing their arguments.
    working = WHERE_RE.replace_all(&working, "").to_string();

    let ids = ID_RE.find_iter(&working).count() as u32;
    working = ID_RE.replace_all(&working, "").to_string();

    let classes = CLASS_RE.find_iter(&working).count() as u32;
    working = CLASS_RE.replace_all(&working, "").to_string();

    working = ATTR_RE.replace_all(&working, "").to_string();

    working = PSEUDO_CLASS_RE.replace_all(&working, "").to_string();

    let mut elements = pseudo_elements + extra_elements + is_elements;
    working = working.replace(['>', '+', '~', ' '], " ");
    for part in working.split_whitespace() {
        if !part.is_empty() && part != "*" {
            elements += 1;
        }
    }
    Specificity::new(
        ids + extra_ids + is_ids,
        classes + extra_classes + is_classes,
        elements,
    )
}

pub fn calculate_specificity(selector: &str) -> Specificity {
    let selector = selector.trim();
    if selector.is_empty() || selector == "*" {
        return Specificity::new(0, 0, 0);
    }

    let selectors: Vec<&str> = selector
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    // Only split by comma for top-level selector lists (e.g., "div, .foo")
    // Don't split for :not(), :is(), etc. with comma-separated arguments -
    // those are handled separately by extract_not_arg and specificity_of_not_arg
    if selectors.len() > 1
        && !selector.contains(":not(")
        && !selector.contains(":is(")
        && !selector.contains(":where(")
    {
        let mut best = Specificity::new(0, 0, 0);
        for sel in selectors {
            let spec = calculate_specificity(sel);
            if compare_specificity(spec, best) > 0 {
                best = spec;
            }
        }
        return best;
    }

    let mut working = selector.to_string();

    let pseudo_elements = PSEUDO_ELEMENT_RE.find_iter(&working).count() as u32;
    working = PSEUDO_ELEMENT_RE.replace_all(&working, "").to_string();

    // Per CSS spec: :not() adds specificity of its argument.
    // Extract and add :not() specificity BEFORE counting IDs/classes in remaining selector.
    let not_arg = extract_not_arg(selector);
    let (not_ids, not_classes, not_elements) = if let Some(arg) = not_arg {
        let spec = specificity_of_not_arg(arg);
        (spec.ids, spec.classes, spec.elements)
    } else {
        (0, 0, 0)
    };
    // Remove :not() blocks so they aren't double-counted by ID/class regexes
    // Handles nested parens via (?:[^()]|\([^)]*\))+
    working = NOT_RE.replace_all(&working, "").to_string();

    // Per CSS spec: :is() adds specificity of its argument.
    // Extract and add :is() specificity BEFORE counting IDs/classes in remaining selector.
    // Only do this if :not() is NOT present at top level (handled by specificity_of_not_arg).
    let is_arg = if not_arg.is_none() {
        extract_is_arg(selector)
    } else {
        None
    };
    let (is_ids, is_classes, is_elements) = if let Some(arg) = is_arg {
        let spec = specificity_of_not_arg(arg);
        (spec.ids, spec.classes, spec.elements)
    } else {
        (0, 0, 0)
    };
    // Remove :is() blocks so they aren't double-counted
    working = IS_RE.replace_all(&working, "").to_string();

    // Per CSS spec: :where() has ZERO specificity - it's completely transparent
    // Remove :where() blocks so they aren't double-counted
    working = WHERE_RE.replace_all(&working, "").to_string();

    let ids = ID_RE.find_iter(&working).count() as u32;
    working = ID_RE.replace_all(&working, "").to_string();

    let classes = CLASS_RE.find_iter(&working).count() as u32;
    working = CLASS_RE.replace_all(&working, "").to_string();

    let attrs = ATTR_RE.find_iter(&working).count() as u32;
    working = ATTR_RE.replace_all(&working, "").to_string();

    let pseudo_classes = PSEUDO_CLASS_RE.find_iter(&working).count() as u32;
    working = PSEUDO_CLASS_RE.replace_all(&working, "").to_string();

    let mut elements = pseudo_elements;
    working = working.replace(['>', '+', '~', ' '], " ");
    for part in working.split_whitespace() {
        if !part.is_empty() && part != "*" {
            elements += 1;
        }
    }

    Specificity::new(
        ids + not_ids + is_ids,
        classes + attrs + pseudo_classes + not_classes + is_classes,
        elements + not_elements + is_elements,
    )
}

pub fn compare_specificity(a: Specificity, b: Specificity) -> i32 {
    if a.ids != b.ids {
        return if a.ids > b.ids { 1 } else { -1 };
    }
    if a.classes != b.classes {
        return if a.classes > b.classes { 1 } else { -1 };
    }
    if a.elements != b.elements {
        return if a.elements > b.elements { 1 } else { -1 };
    }
    0
}

pub fn format_specificity(spec: Specificity) -> String {
    format!("({},{},{})", spec.ids, spec.classes, spec.elements)
}

pub fn matches_context(
    definition_selector: &str,
    usage_context: &str,
    dom_tree: Option<&DomTree>,
    dom_node: Option<&DOMNodeInfo>,
) -> bool {
    if let (Some(tree), Some(node)) = (dom_tree, dom_node) {
        if let Some(node_index) = node.node_index {
            return tree.matches_selector(node_index, definition_selector);
        }
    }

    let def_trim = definition_selector.trim();
    let usage_trim = usage_context.trim();

    if def_trim == ":root" {
        return true;
    }

    if def_trim == usage_trim {
        return true;
    }

    let def_parts: Vec<&str> = def_trim.split(&[' ', '>', '+', '~'][..]).collect();
    let usage_parts: Vec<&str> = usage_trim.split(&[' ', '>', '+', '~'][..]).collect();

    def_parts.iter().any(|def_part| {
        usage_parts.iter().any(|usage_part| {
            !def_part.is_empty()
                && !usage_part.is_empty()
                && (usage_part.contains(def_part) || def_part.contains(usage_part))
        })
    })
}

/// Sort variables by cascade rules (winner first):
/// !important > inline > specificity > source order (later wins)
pub fn sort_by_cascade(variables: &mut [CssVariable]) {
    variables.sort_by(|a, b| {
        if a.important != b.important {
            return if a.important {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }

        if a.inline != b.inline {
            return if a.inline {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }

        let spec_a = calculate_specificity(&a.selector);
        let spec_b = calculate_specificity(&b.selector);
        let spec_cmp = compare_specificity(spec_a, spec_b);
        if spec_cmp != 0 {
            return if spec_cmp > 0 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }

        b.source_position.cmp(&a.source_position)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_specificity_calculation() {
        let root = calculate_specificity(":root");
        assert_eq!(root.ids, 0);
        assert_eq!(root.classes, 1);
        assert_eq!(root.elements, 0);
        assert_eq!(format_specificity(root), "(0,1,0)");
    }

    #[test]
    fn element_selector_specificity() {
        let div = calculate_specificity("div");
        assert_eq!(div.ids, 0);
        assert_eq!(div.classes, 0);
        assert_eq!(div.elements, 1);
    }

    #[test]
    fn class_selector_specificity() {
        let class = calculate_specificity(".button");
        assert_eq!(class.ids, 0);
        assert_eq!(class.classes, 1);
        assert_eq!(class.elements, 0);
    }

    #[test]
    fn id_selector_specificity() {
        let id = calculate_specificity("#main");
        assert_eq!(id.ids, 1);
        assert_eq!(id.classes, 0);
        assert_eq!(id.elements, 0);
    }

    #[test]
    fn complex_selector_specificity() {
        let spec = calculate_specificity("div.button#submit");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 1);
    }

    #[test]
    fn specificity_comparison() {
        let root = calculate_specificity(":root");
        let div = calculate_specificity("div");
        let cls = calculate_specificity(".button");
        let id = calculate_specificity("#main");

        assert_eq!(compare_specificity(div, root), -1);
        assert_eq!(compare_specificity(cls, div), 1);
        assert_eq!(compare_specificity(id, cls), 1);
        assert_eq!(compare_specificity(root, root), 0);
    }

    #[test]
    fn context_matching_basics() {
        assert!(matches_context(":root", "div", None, None));
        assert!(matches_context("div", "div", None, None));
        assert!(matches_context(":root", ".button", None, None));
    }

    #[test]
    fn not_with_class_specificity() {
        // :not(.foo) has specificity of .foo → (0,1,0)
        let spec = calculate_specificity(":not(.foo)");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn not_with_id_specificity() {
        // :not(#bar) has specificity of #bar → (1,0,0)
        let spec = calculate_specificity(":not(#bar)");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 0);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn not_with_multiple_args_specificity() {
        // :not(.foo, #bar) → take max across args → (1,1,0)
        let spec = calculate_specificity(":not(.foo, #bar)");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn not_with_complex_selector_specificity() {
        // :not(.foo.bar) → specificity of .foo.bar = 2 classes → (0,2,0)
        let spec = calculate_specificity(":not(.foo.bar)");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 2);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn not_with_element_specificity() {
        // :not(div) → (0,0,1)
        let spec = calculate_specificity(":not(div)");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 0);
        assert_eq!(spec.elements, 1);
    }

    #[test]
    fn not_preserves_other_selectors() {
        // .foo:not(#bar) → (1,1,0)
        let spec = calculate_specificity(".foo:not(#bar)");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn not_nested_is_specificity() {
        // :not(:is(.foo)) should return specificity of .foo → (0,1,0)
        // The :not() takes specificity of its argument :is(.foo),
        // which in turn takes specificity of .foo
        let spec = calculate_specificity(":not(:is(.foo))");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn where_zero_specificity() {
        // :where() has zero specificity per CSS spec - it's transparent
        let spec = calculate_specificity(":where(.foo)");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 0);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn where_nested_in_not() {
        // :not(:where(.foo)) should return zero specificity from :where()
        let spec = calculate_specificity(":not(:where(.foo))");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 0);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn is_with_class_specificity() {
        // :is(.foo) should return specificity of .foo → (0,1,0)
        let spec = calculate_specificity(":is(.foo)");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn is_with_id_specificity() {
        // :is(#bar) should return specificity of #bar → (1,0,0)
        let spec = calculate_specificity(":is(#bar)");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 0);
        assert_eq!(spec.elements, 0);
    }

    #[test]
    fn is_with_multiple_args_specificity() {
        // :is(.foo, #bar) → take max across args → (1,1,0)
        let spec = calculate_specificity(":is(.foo, #bar)");
        assert_eq!(spec.ids, 1);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }
}
