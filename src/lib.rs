// Library interface for css-variable-lsp
// This allows integration tests and external usage

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod color;
pub mod completion_context;
pub mod document_kind;
pub mod dom_tree;
pub mod flags;
pub mod lsp_server;
pub mod manager;
pub mod parsers;
pub mod path_display;
pub mod runtime_config;
pub mod specificity;
pub mod text_utils;
pub mod types;
pub mod workspace;

// Re-export commonly used types
pub use workspace::ScanStats;
