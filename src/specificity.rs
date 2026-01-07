use regex::Regex;

use crate::dom_tree::DomTree;
use crate::types::{CssVariable, DOMNodeInfo};

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
    if selectors.len() > 1 {
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
    let pseudo_element_re = Regex::new(r"::[a-zA-Z-]+").unwrap();
    let pseudo_elements = pseudo_element_re.find_iter(&working).count() as u32;
    working = pseudo_element_re.replace_all(&working, "").to_string();

    let id_re = Regex::new(r"#[a-zA-Z0-9_-]+").unwrap();
    let ids = id_re.find_iter(&working).count() as u32;
    working = id_re.replace_all(&working, "").to_string();

    let class_re = Regex::new(r"\.[a-zA-Z0-9_-]+").unwrap();
    let classes = class_re.find_iter(&working).count() as u32;
    working = class_re.replace_all(&working, "").to_string();

    let attr_re = Regex::new(r#"\[(?:[^\]"']|"[^"]*"|'[^']*')*\]"#).unwrap();
    let attrs = attr_re.find_iter(&working).count() as u32;
    working = attr_re.replace_all(&working, "").to_string();

    let pseudo_class_re = Regex::new(r":[a-zA-Z-]+(\([^)]*\))?").unwrap();
    let pseudo_classes = pseudo_class_re.find_iter(&working).count() as u32;
    working = pseudo_class_re.replace_all(&working, "").to_string();

    let mut elements = pseudo_elements;
    working = working.replace(['>', '+', '~', ' '], " ");
    for part in working.split_whitespace() {
        if !part.is_empty() && part != "*" {
            elements += 1;
        }
    }

    Specificity::new(ids, classes + attrs + pseudo_classes, elements)
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
}
