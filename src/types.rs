use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::{Position, Range, Url};

use crate::runtime_config::RuntimeConfig;

/// Represents a CSS variable definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CssVariable {
    /// Variable name (e.g., "--primary-color")
    pub name: String,

    /// Variable value (e.g., "#3b82f6")
    pub value: String,

    /// Document URI where the variable is defined
    pub uri: Url,

    /// Range of the entire declaration (e.g., "--foo: red")
    pub range: Range,

    /// Range of just the variable name (e.g., "--foo")
    pub name_range: Option<Range>,

    /// Range of just the value part (e.g., "red")
    pub value_range: Option<Range>,

    /// CSS selector where this variable is defined (e.g., ":root", "div", ".class")
    pub selector: String,

    /// Whether this definition uses !important
    pub important: bool,

    /// Whether this definition is from an inline style attribute
    pub inline: bool,

    /// Character position in file (for source order in cascade)
    pub source_position: usize,
}

/// Represents a CSS variable usage (var() call)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CssVariableUsage {
    /// Variable name being used
    pub name: String,

    /// Document URI where the variable is used
    pub uri: Url,

    /// Range of the var() call
    pub range: Range,

    /// Range of just the variable name in var()
    pub name_range: Option<Range>,

    /// CSS selector context where variable is used
    pub usage_context: String,

    /// DOM node info if usage is in HTML (for inline styles)
    pub dom_node: Option<DOMNodeInfo>,
}

/// Information about a DOM node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DOMNodeInfo {
    /// Tag name (e.g., "div", "span")
    pub tag: String,

    /// ID attribute if present
    pub id: Option<String>,

    /// Classes
    pub classes: Vec<String>,

    /// Position in document
    pub position: usize,

    /// Internal node index for selector matching
    pub node_index: Option<usize>,
}

/// Configuration settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// File patterns to scan for CSS variables
    pub lookup_files: Vec<String>,

    /// Glob patterns to ignore
    pub ignore_globs: Vec<String>,

    /// Enable color provider
    pub enable_color_provider: bool,

    /// Only show colors on variables (not inline values)
    pub color_only_on_variables: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lookup_files: vec![
                "**/*.css".to_string(),
                "**/*.scss".to_string(),
                "**/*.sass".to_string(),
                "**/*.less".to_string(),
                "**/*.html".to_string(),
                "**/*.vue".to_string(),
                "**/*.svelte".to_string(),
                "**/*.astro".to_string(),
                "**/*.ripple".to_string(),
            ],
            ignore_globs: vec![
                "**/node_modules/**".to_string(),
                "**/dist/**".to_string(),
                "**/out/**".to_string(),
                "**/.git/**".to_string(),
            ],
            enable_color_provider: true,
            color_only_on_variables: false,
        }
    }
}

impl Config {
    pub fn from_runtime(runtime: &RuntimeConfig) -> Self {
        let mut config = Config::default();
        if let Some(lookup) = &runtime.lookup_files {
            if !lookup.is_empty() {
                config.lookup_files = lookup.clone();
            }
        }
        if let Some(ignore) = &runtime.ignore_globs {
            if !ignore.is_empty() {
                config.ignore_globs = ignore.clone();
            }
        }
        config.enable_color_provider = runtime.enable_color_provider;
        config.color_only_on_variables = runtime.color_only_on_variables;
        config
    }
}

/// Helper to convert byte offset to LSP Position
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut character = 0;

    for (idx, ch) in text.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }

    Position::new(line, character)
}

/// Helper to convert LSP Position to byte offset
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut character = 0;

    for (idx, ch) in text.char_indices() {
        if line == position.line && character == position.character {
            return Some(idx);
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }

    if line == position.line && character == position.character {
        Some(text.len())
    } else {
        None
    }
}
