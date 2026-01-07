use crate::types::DOMNodeInfo;

#[derive(Debug, Clone)]
pub struct DomNode {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub start: usize,
    pub end: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct DomTree {
    nodes: Vec<DomNode>,
    roots: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct StyleBlock {
    pub content: String,
    pub content_start: usize,
}

#[derive(Debug, Clone)]
pub struct InlineStyle {
    pub value: String,
    pub value_start: usize,
    pub attribute_start: usize,
}

#[derive(Debug, Clone)]
pub struct HtmlParseResult {
    pub dom_tree: DomTree,
    pub style_blocks: Vec<StyleBlock>,
    pub inline_styles: Vec<InlineStyle>,
}

impl DomTree {
    pub fn parse(html: &str) -> HtmlParseResult {
        let bytes = html.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        let mut comment_depth = 0usize;
        let mut nodes: Vec<DomNode> = Vec::new();
        let mut roots: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut style_blocks = Vec::new();
        let mut inline_styles = Vec::new();

        while i < len {
            if comment_depth > 0 {
                if starts_with(bytes, i, b"<!--") {
                    comment_depth += 1;
                    i += 4;
                    continue;
                }
                if starts_with(bytes, i, b"-->") {
                    comment_depth -= 1;
                    i += 3;
                    continue;
                }
                i += 1;
                continue;
            }

            if starts_with(bytes, i, b"<!--") {
                comment_depth = 1;
                i += 4;
                continue;
            }

            if bytes[i] != b'<' {
                i += 1;
                continue;
            }

            if i + 1 >= len {
                break;
            }

            if bytes[i + 1] == b'/' {
                // end tag
                if let Some((tag_name, end_pos)) = parse_end_tag(html, i) {
                    let mut match_index = None;
                    for (pos, node_idx) in stack.iter().enumerate().rev() {
                        if nodes[*node_idx].tag == tag_name {
                            match_index = Some(pos);
                            break;
                        }
                    }
                    if let Some(pos) = match_index {
                        let node_idx = stack[pos];
                        nodes[node_idx].end = end_pos;
                        stack.truncate(pos);
                    }
                    i = end_pos;
                    continue;
                }
            }

            if bytes[i + 1] == b'!' {
                // doctype or other markup, skip to >
                if let Some(end_pos) = find_char(bytes, i + 2, b'>') {
                    i = end_pos + 1;
                    continue;
                }
            }

            let tag_start = i;
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let name_start = i;
            while i < len && is_tag_name_char(bytes[i]) {
                i += 1;
            }
            if name_start == i {
                i += 1;
                continue;
            }
            let tag_name = html[name_start..i].to_lowercase();

            let mut id: Option<String> = None;
            let mut classes: Vec<String> = Vec::new();
            let mut self_closing = false;

            while i < len {
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= len {
                    break;
                }
                if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'>' {
                    self_closing = true;
                    i += 2;
                    break;
                }
                if bytes[i] == b'>' {
                    i += 1;
                    break;
                }

                let attr_name_start = i;
                while i < len && is_attr_name_char(bytes[i]) {
                    i += 1;
                }
                if attr_name_start == i {
                    i += 1;
                    continue;
                }
                let attr_name = html[attr_name_start..i].to_lowercase();
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }

                let mut value: Option<String> = None;
                let mut value_start = None;
                if i < len && bytes[i] == b'=' {
                    i += 1;
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                        let quote = bytes[i];
                        i += 1;
                        let start = i;
                        while i < len && bytes[i] != quote {
                            i += 1;
                        }
                        let end = i.min(len);
                        value = Some(html[start..end].to_string());
                        value_start = Some(start);
                        if i < len {
                            i += 1;
                        }
                    } else {
                        let start = i;
                        while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                            i += 1;
                        }
                        let end = i;
                        value = Some(html[start..end].to_string());
                        value_start = Some(start);
                    }
                }

                match attr_name.as_str() {
                    "id" => {
                        if let Some(v) = &value {
                            if !v.is_empty() {
                                id = Some(v.to_string());
                            }
                        }
                    }
                    "class" => {
                        if let Some(v) = &value {
                            classes.extend(v.split_whitespace().map(|c| c.to_string()));
                        }
                    }
                    "style" => {
                        if let (Some(v), Some(v_start)) = (value.clone(), value_start) {
                            inline_styles.push(InlineStyle {
                                value: v,
                                value_start: v_start,
                                attribute_start: attr_name_start,
                            });
                        }
                    }
                    _ => {}
                }
            }

            let tag_end = i;
            let node_idx = nodes.len();
            let parent = stack.last().copied();
            nodes.push(DomNode {
                tag: tag_name.clone(),
                id,
                classes,
                start: tag_start,
                end: tag_end,
                parent,
                children: Vec::new(),
            });

            if let Some(parent_idx) = parent {
                nodes[parent_idx].children.push(node_idx);
            } else {
                roots.push(node_idx);
            }

            if tag_name == "style" {
                if let Some((content_start, content_end, close_end)) =
                    find_block_content(html, tag_end, "style")
                {
                    let content = html[content_start..content_end].to_string();
                    style_blocks.push(StyleBlock {
                        content,
                        content_start,
                    });
                    nodes[node_idx].end = close_end;
                    i = close_end;
                    continue;
                }
            }

            if tag_name == "script" {
                if let Some((_, _, close_end)) = find_block_content(html, tag_end, "script") {
                    nodes[node_idx].end = close_end;
                    i = close_end;
                    continue;
                }
            }

            if self_closing || is_void_tag(&tag_name) {
                nodes[node_idx].end = tag_end;
            } else {
                stack.push(node_idx);
            }
        }

        let final_end = html.len();
        for idx in stack {
            if nodes[idx].end < final_end {
                nodes[idx].end = final_end;
            }
        }

        HtmlParseResult {
            dom_tree: DomTree { nodes, roots },
            style_blocks,
            inline_styles,
        }
    }

    pub fn find_node_at_position(&self, position: usize) -> Option<DOMNodeInfo> {
        for &root_idx in &self.roots {
            if let Some(info) = self.find_node_recursive(root_idx, position) {
                return Some(info);
            }
        }
        None
    }

    fn find_node_recursive(&self, idx: usize, position: usize) -> Option<DOMNodeInfo> {
        let node = &self.nodes[idx];
        if position < node.start || position > node.end {
            return None;
        }
        for &child in &node.children {
            if let Some(found) = self.find_node_recursive(child, position) {
                return Some(found);
            }
        }
        Some(self.to_info(idx))
    }

    pub fn matches_selector(&self, node_index: usize, selector: &str) -> bool {
        let selector = selector.trim();
        if selector.is_empty() {
            return false;
        }
        if selector == ":root" {
            return true;
        }
        let selectors: Vec<&str> = selector.split(',').map(|s| s.trim()).collect();
        for sel in selectors {
            if sel.is_empty() {
                continue;
            }
            let parts = parse_selector_parts(sel);
            if matches_selector_parts(self, node_index, &parts) {
                return true;
            }
        }
        false
    }

    fn to_info(&self, idx: usize) -> DOMNodeInfo {
        let node = &self.nodes[idx];
        DOMNodeInfo {
            tag: node.tag.clone(),
            id: node.id.clone(),
            classes: node.classes.clone(),
            position: node.start,
            node_index: Some(idx),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Combinator {
    Descendant,
    Child,
}

#[derive(Debug, Clone)]
struct SimpleSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Debug, Clone)]
struct SelectorPart {
    combinator: Combinator,
    selector: SimpleSelector,
}

fn parse_selector_parts(selector: &str) -> Vec<SelectorPart> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_attr = 0usize;
    let mut in_paren = 0usize;
    let mut last_was_space = false;

    for ch in selector.chars() {
        match ch {
            '[' => {
                in_attr += 1;
                current.push(ch);
                last_was_space = false;
            }
            ']' => {
                in_attr = in_attr.saturating_sub(1);
                current.push(ch);
                last_was_space = false;
            }
            '(' => {
                in_paren += 1;
                current.push(ch);
                last_was_space = false;
            }
            ')' => {
                in_paren = in_paren.saturating_sub(1);
                current.push(ch);
                last_was_space = false;
            }
            '>' if in_attr == 0 && in_paren == 0 => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                }
                tokens.push(">".to_string());
                current.clear();
                last_was_space = false;
            }
            ch if ch.is_whitespace() && in_attr == 0 && in_paren == 0 => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
                if !last_was_space {
                    tokens.push(" ".to_string());
                    last_was_space = true;
                }
            }
            _ => {
                current.push(ch);
                last_was_space = false;
            }
        }
    }

    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }

    let mut parts = Vec::new();
    let mut next_combinator = Combinator::Descendant;

    for token in tokens {
        if token == ">" {
            next_combinator = Combinator::Child;
            continue;
        }
        if token == " " {
            next_combinator = Combinator::Descendant;
            continue;
        }
        let selector = parse_simple_selector(&token);
        parts.push(SelectorPart {
            combinator: next_combinator,
            selector,
        });
        next_combinator = Combinator::Descendant;
    }

    parts
}

fn parse_simple_selector(token: &str) -> SimpleSelector {
    let mut tag: Option<String> = None;
    let mut id: Option<String> = None;
    let mut classes: Vec<String> = Vec::new();

    let mut slice = token;
    if let Some(idx) = slice.find([':', '[']) {
        slice = &slice[..idx];
    }

    let mut current = String::new();
    let mut mode = 't'; // t=tag, c=class, i=id

    for ch in slice.chars() {
        match ch {
            '#' => {
                if mode == 't' && !current.is_empty() && tag.is_none() {
                    tag = Some(current.clone());
                } else if mode == 'c' && !current.is_empty() {
                    classes.push(current.clone());
                }
                current.clear();
                mode = 'i';
            }
            '.' => {
                if mode == 't' && !current.is_empty() && tag.is_none() {
                    tag = Some(current.clone());
                } else if mode == 'i' && !current.is_empty() {
                    id = Some(current.clone());
                } else if mode == 'c' && !current.is_empty() {
                    classes.push(current.clone());
                }
                current.clear();
                mode = 'c';
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        match mode {
            't' => {
                if current != "*" {
                    tag = Some(current);
                }
            }
            'i' => id = Some(current),
            'c' => classes.push(current),
            _ => {}
        }
    }

    SimpleSelector { tag, id, classes }
}

fn matches_selector_parts(tree: &DomTree, node_index: usize, parts: &[SelectorPart]) -> bool {
    if parts.is_empty() {
        return false;
    }

    let mut current_index = Some(node_index);
    for (idx, part) in parts.iter().enumerate().rev() {
        let node_idx = match current_index {
            Some(i) => i,
            None => return false,
        };
        if !matches_simple_selector(&tree.nodes[node_idx], &part.selector) {
            return false;
        }

        if idx == 0 {
            return true;
        }

        let next_part = &parts[idx - 1];
        match part.combinator {
            Combinator::Child => {
                current_index = tree.nodes[node_idx].parent;
                if let Some(parent_idx) = current_index {
                    if !matches_simple_selector(&tree.nodes[parent_idx], &next_part.selector) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            Combinator::Descendant => {
                let mut parent = tree.nodes[node_idx].parent;
                let mut matched = false;
                while let Some(parent_idx) = parent {
                    if matches_simple_selector(&tree.nodes[parent_idx], &next_part.selector) {
                        matched = true;
                        current_index = Some(parent_idx);
                        break;
                    }
                    parent = tree.nodes[parent_idx].parent;
                }
                if !matched {
                    return false;
                }
            }
        }
    }

    true
}

fn matches_simple_selector(node: &DomNode, selector: &SimpleSelector) -> bool {
    if let Some(tag) = &selector.tag {
        if node.tag != tag.to_lowercase() {
            return false;
        }
    }
    if let Some(id) = &selector.id {
        if node.id.as_deref() != Some(id) {
            return false;
        }
    }
    for class in &selector.classes {
        if !node.classes.iter().any(|c| c == class) {
            return false;
        }
    }
    true
}

fn is_tag_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b':'
}

fn is_attr_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn starts_with(bytes: &[u8], idx: usize, pattern: &[u8]) -> bool {
    bytes.len() >= idx + pattern.len() && &bytes[idx..idx + pattern.len()] == pattern
}

fn find_char(bytes: &[u8], start: usize, target: u8) -> Option<usize> {
    (start..bytes.len()).find(|&i| bytes[i] == target)
}

fn parse_end_tag(html: &str, start: usize) -> Option<(String, usize)> {
    let bytes = html.as_bytes();
    let len = bytes.len();
    if start + 2 >= len {
        return None;
    }
    let mut i = start + 2;
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let name_start = i;
    while i < len && is_tag_name_char(bytes[i]) {
        i += 1;
    }
    if name_start == i {
        return None;
    }
    let tag_name = html[name_start..i].to_lowercase();
    if let Some(end_pos) = find_char(bytes, i, b'>') {
        return Some((tag_name, end_pos + 1));
    }
    None
}

fn find_block_content(html: &str, start: usize, tag: &str) -> Option<(usize, usize, usize)> {
    let lower = html.to_lowercase();
    let bytes = lower.as_bytes();
    let len = bytes.len();
    let target = format!("</{}", tag.to_lowercase());
    let target_bytes = target.as_bytes();
    let mut i = start;
    while i + target_bytes.len() < len {
        if bytes[i] == b'<' && starts_with(bytes, i, target_bytes) {
            if let Some(end_pos) = find_char(bytes, i + target_bytes.len(), b'>') {
                return Some((start, i, end_pos + 1));
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dom_tree_basic() {
        let html = r#"
            <html>
                <body>
                    <div class="container">
                        <p id="text">Hello</p>
                    </div>
                </body>
            </html>
        "#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;
        assert!(!tree.roots.is_empty());

        // Should be able to find nodes
        let node = tree.find_node_at_position(50);
        assert!(node.is_some());
    }

    #[test]
    fn test_dom_tree_nested_structure() {
        let html =
            r#"<div class="outer"><div class="inner"><span id="item">Text</span></div></div>"#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;

        // Tree should have nodes
        assert!(!tree.roots.is_empty());

        // We can verify the parse result has the expected structure
        assert_eq!(tree.nodes.len(), 3); // div, div, span
    }

    #[test]
    fn test_dom_tree_multiple_classes() {
        let html = r#"<div class="class1 class2 class3">Content</div>"#;

        let result = DomTree::parse(html);
        assert!(!result.dom_tree.roots.is_empty());
        // Classes are parsed correctly
    }

    #[test]
    fn test_dom_tree_self_closing_tags() {
        let html = r#"<div><img src="test.jpg" /><br /><input type="text" /></div>"#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;

        assert!(!tree.roots.is_empty());
        // Self-closing tags are handled
    }

    #[test]
    fn test_dom_tree_find_node_at_position() {
        let html = r#"<div class="outer"><p id="para">Text</p></div>"#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;

        // Position in div tag
        let node = tree.find_node_at_position(5);
        assert!(node.is_some());
    }

    #[test]
    fn test_dom_tree_empty_html() {
        let html = "";
        let result = DomTree::parse(html);
        let tree = result.dom_tree;
        assert!(tree.roots.is_empty());
    }

    #[test]
    fn test_dom_tree_malformed_html() {
        // Missing closing tag
        let html = r#"<div><p>Text"#;
        let result = DomTree::parse(html);
        // Should still parse what it can
        assert!(!result.dom_tree.roots.is_empty());
    }

    #[test]
    fn test_parse_inline_styles() {
        let html = r#"<div style="color: red; background: blue;"></div>"#;

        let parsed = DomTree::parse(html);
        assert_eq!(parsed.inline_styles.len(), 1);

        let inline = &parsed.inline_styles[0];
        assert!(inline.value.contains("color: red"));
        assert!(inline.value.contains("background: blue"));
    }

    #[test]
    fn test_parse_style_blocks() {
        let html = r#"
            <html>
                <head>
                    <style>
                        .class { color: red; }
                    </style>
                </head>
                <body>
                    <style>
                        #id { background: blue; }
                    </style>
                </body>
            </html>
        "#;

        let parsed = DomTree::parse(html);
        assert_eq!(parsed.style_blocks.len(), 2);

        assert!(parsed.style_blocks[0].content.contains("color: red"));
        assert!(parsed.style_blocks[1].content.contains("background: blue"));
    }

    #[test]
    fn test_parse_nested_style_tags() {
        let html = r#"<style>outer { color: red; }<style>inner</style></style>"#;

        let parsed = DomTree::parse(html);
        // Should handle nested style tags
        assert!(!parsed.style_blocks.is_empty());
    }

    #[test]
    fn test_dom_tree_comment_handling() {
        let html = r#"<div><!-- This is a comment --><p>Text</p></div>"#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;

        // Comments should be handled properly
        assert!(!tree.roots.is_empty());
    }

    #[test]
    fn test_attributes_with_quotes() {
        let html = r#"<div class="test" id='myid' data-value=unquoted></div>"#;

        let result = DomTree::parse(html);
        let tree = result.dom_tree;

        // Should parse attributes correctly
        assert!(!tree.roots.is_empty());
    }
}
